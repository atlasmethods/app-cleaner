//! Executes rule targets in one of two modes that share the same scanning code:
//! [`Mode::Analyze`] (pure read) and [`Mode::Clean`] (delete). Sharing the code is what
//! guarantees that a clean removes exactly what the preceding analysis reported.

use globset::{Glob, GlobBuilder, GlobMatcher, GlobSet, GlobSetBuilder};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use walkdir::WalkDir;

use crate::ctx::{Ctx, Os};
use crate::error::Result;
use crate::features::cleaner::model::{
    PathErr, MAX_ANALYZE_ERRORS, MAX_CLEAN_FAILURES, MAX_SAMPLES,
};
use crate::features::cleaner::rules::{
    CommandTarget, CookiesTarget, FileTarget, FilesTarget, RegistryTarget, Rule, SqliteTarget,
    Target,
};
use crate::features::cleaner::sqlite::{self, CookieSelect, DbError};
use crate::features::cleaner::template;
use crate::features::settings::Settings;
use crate::job::Job;
use crate::safety::{io_message, SafeDeleter, Safety};

const CASE_INSENSITIVE: bool = cfg!(any(windows, target_os = "macos"));

/// PowerShell scripts for the Windows Recycle Bin (run through the command runner).
pub const RECYCLE_SIZE_SCRIPT: &str =
    "$s=(New-Object -ComObject Shell.Application).NameSpace(10); $c=0; $b=0; \
foreach($i in $s.Items()){ $c++; $b+=[int64]$i.Size }; \"$c $b\"";
pub const RECYCLE_CLEAR_SCRIPT: &str = "Clear-RecycleBin -Force -ErrorAction SilentlyContinue";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Analyze,
    Clean,
}

/// Everything a run needs, computed once.
pub struct Engine<'a> {
    pub ctx: &'a Ctx,
    pub settings: &'a Settings,
    pub safety: Arc<Safety>,
    /// Normalized cookie keep-list.
    pub keep: Vec<String>,
    pub now: SystemTime,
    pub mode: Mode,
    /// Overwrite files before deleting (Clean mode only).
    pub secure_passes: Option<u32>,
}

/// Accumulated result of running all targets of one rule.
#[derive(Debug, Default)]
pub struct Outcome {
    pub files: u64,
    pub bytes: u64,
    pub rows: u64,
    pub samples: Vec<String>,
    pub errors: Vec<PathErr>,
    pub error_count: u64,
    pub actions: Vec<String>,
    /// Databases that were locked.
    pub in_use: u64,
    /// Targets that could not run on this platform.
    pub unsupported: u64,
    /// Targets that did real work or found something to do (used for `skipped` decisions).
    pub touched: u64,
    /// Paths already accepted by an earlier target of the same rule (overlap guard, so
    /// nothing is counted or deleted twice).
    seen: std::collections::HashSet<PathBuf>,
}

impl Outcome {
    fn error(&mut self, mode: Mode, path: &Path, msg: impl Into<String>) {
        self.error_count += 1;
        let cap = if mode == Mode::Analyze {
            MAX_ANALYZE_ERRORS
        } else {
            MAX_CLEAN_FAILURES
        };
        if self.errors.len() < cap {
            self.errors.push(PathErr {
                path: path.to_string_lossy().into_owned(),
                message: msg.into(),
            });
        }
    }
    fn sample(&mut self, p: &Path) {
        if self.samples.len() < MAX_SAMPLES {
            self.samples.push(p.to_string_lossy().into_owned());
        }
    }
}

impl Engine<'_> {
    /// Run every target of `rule` that applies to the current OS.
    ///
    /// Results accumulate in `out` even when the run is cancelled part-way (`Err`), so the
    /// caller can still report what was already removed.
    pub fn run_rule(&self, rule: &Rule, job: &Job, out: &mut Outcome) -> Result<()> {
        for t in rule.targets_for(self.ctx.env.os) {
            job.check_cancelled()?;
            match t {
                Target::Files(f) => self.files(f, job, out)?,
                Target::File(f) => self.file(f, out),
                Target::Sqlite(s) => self.sqlite(s, out),
                Target::Cookies(c) => self.cookies(c, out),
                Target::Command(c) => self.command(c, out),
                Target::Trash(_) => self.trash(job, out)?,
                Target::Registry(r) => self.registry(r, out),
            }
        }
        Ok(())
    }

    fn deleter(&self, base: &Path) -> Result<SafeDeleter> {
        let d = SafeDeleter::with_safety(self.safety.clone(), base)?;
        Ok(match (self.mode, self.secure_passes) {
            (Mode::Clean, Some(p)) => d.with_secure(p),
            _ => d,
        })
    }

    // ------------------------------------------------------------ files

    /// Existing real directories a `files` target refers to.
    fn bases(&self, t: &FilesTarget, out: &mut Outcome) -> Vec<PathBuf> {
        let mut bases = Vec::new();
        let candidates: Vec<(PathBuf, std::fs::Metadata)> = match &t.literal_base {
            Some(p) => match std::fs::symlink_metadata(p) {
                Ok(m) => vec![(p.clone(), m)],
                Err(_) => Vec::new(),
            },
            None => template::resolve(&self.ctx.env, &t.base)
                .into_iter()
                .map(|f| (f.path, f.meta))
                .collect(),
        };
        for (p, meta) in candidates {
            let ft = meta.file_type();
            if ft.is_symlink() {
                // Never follow a symlinked base: it could point anywhere.
                out.error(self.mode, &p, "skipped: folder is a symbolic link");
            } else if ft.is_dir() {
                if self.safety.excludes.is_excluded(&p) {
                    continue;
                }
                bases.push(p);
            }
        }
        bases
    }

    fn files(&self, t: &FilesTarget, job: &Job, out: &mut Outcome) -> Result<()> {
        let matcher = compile_pattern(&t.pattern);
        let skip = compile_skip(&t.skip_names);
        let min_age = t
            .min_age_hours
            .or(t
                .min_age_from_settings
                .then_some(self.settings.temp_min_age_hours))
            .map(|h| Duration::from_secs(u64::from(h) * 3600));

        for base in self.bases(t, out) {
            job.check_cancelled()?;
            let deleter = match self.deleter(&base) {
                Ok(d) => d,
                Err(e) => {
                    out.error(self.mode, &base, e.message);
                    continue;
                }
            };
            let mut cands: Vec<(PathBuf, u64)> = Vec::new();
            let mut dirs: Vec<PathBuf> = Vec::new();
            let walker = WalkDir::new(&base)
                .follow_links(false)
                .same_file_system(true)
                .min_depth(1)
                .max_depth(if t.recursive { usize::MAX } else { 1 });
            let excludes = &self.safety.excludes;
            let entries = walker
                .into_iter()
                .filter_entry(|e| !skip.is_match(e.file_name()) && !excludes.is_excluded(e.path()));
            for entry in entries {
                if cands.len() & 1023 == 0 {
                    job.check_cancelled()?;
                }
                let entry = match entry {
                    Ok(e) => e,
                    Err(e) => {
                        let p = e.path().unwrap_or(&base).to_path_buf();
                        let msg = e
                            .io_error()
                            .map(io_message)
                            .unwrap_or_else(|| e.to_string());
                        out.error(self.mode, &p, msg);
                        continue;
                    }
                };
                let ft = entry.file_type();
                let Ok(meta) = entry.metadata() else { continue };
                if ft.is_dir() {
                    if t.remove_empty_dirs
                        && filters_pass(&meta, min_age, t.owned_by_user, self.now)
                    {
                        dirs.push(entry.path().to_path_buf());
                    }
                    continue;
                }
                if !(ft.is_file() || ft.is_symlink()) {
                    continue; // sockets, pipes, devices: never touched
                }
                if !matcher.is_match(entry.file_name()) {
                    continue;
                }
                if !filters_pass(&meta, min_age, t.owned_by_user, self.now) {
                    continue;
                }
                match deleter.check(entry.path()) {
                    Ok(_) => {
                        if !out.seen.insert(entry.path().to_path_buf()) {
                            continue;
                        }
                        let bytes = if ft.is_file() { meta.len() } else { 0 };
                        cands.push((entry.path().to_path_buf(), bytes));
                    }
                    Err(e) if e.is_excluded() => {}
                    Err(e) => out.error(self.mode, entry.path(), e.message()),
                }
            }

            match self.mode {
                Mode::Analyze => {
                    for (p, bytes) in &cands {
                        out.files += 1;
                        out.bytes += bytes;
                        out.sample(p);
                    }
                }
                Mode::Clean => {
                    for (i, (p, _)) in cands.iter().enumerate() {
                        if i & 255 == 0 {
                            job.check_cancelled()?;
                        }
                        match deleter.remove_file(p) {
                            Ok(bytes) => {
                                out.files += 1;
                                out.bytes += bytes;
                            }
                            Err(e) if e.is_excluded() || e.is_not_found() => {}
                            Err(e) => out.error(self.mode, p, e.message()),
                        }
                    }
                    // Deepest first; `remove_dir` only succeeds on empty directories.
                    for d in dirs.iter().rev() {
                        let _ = deleter.remove_empty_dir(d);
                    }
                    if !t.keep_base {
                        if let Some(parent) = base.parent() {
                            if let Ok(pd) = SafeDeleter::with_safety(self.safety.clone(), parent) {
                                let _ = pd.remove_empty_dir(&base);
                            }
                        }
                    }
                }
            }
            out.touched += 1;
        }
        Ok(())
    }

    // ------------------------------------------------------------ single files

    fn file(&self, t: &FileTarget, out: &mut Outcome) {
        for f in template::resolve(&self.ctx.env, &t.path) {
            let ft = f.meta.file_type();
            if !(ft.is_file() || ft.is_symlink()) {
                continue;
            }
            let Some(parent) = f.path.parent() else {
                continue;
            };
            // The parent of a single file may legitimately be a protected folder
            // (e.g. C:\Windows); the file itself is still checked.
            let deleter = match SafeDeleter::for_selection(self.safety.clone(), parent) {
                Ok(d) => match (self.mode, self.secure_passes) {
                    (Mode::Clean, Some(p)) => d.with_secure(p),
                    _ => d,
                },
                Err(e) => {
                    out.error(self.mode, &f.path, e.message);
                    continue;
                }
            };
            match self.mode {
                Mode::Analyze => match deleter.check(&f.path) {
                    Ok(r) => {
                        out.files += 1;
                        if r.meta.file_type().is_file() {
                            out.bytes += r.meta.len();
                        }
                        out.sample(&f.path);
                    }
                    Err(e) if e.is_excluded() => {}
                    Err(e) => out.error(self.mode, &f.path, e.message()),
                },
                Mode::Clean => match deleter.remove_file(&f.path) {
                    Ok(bytes) => {
                        out.files += 1;
                        out.bytes += bytes;
                    }
                    Err(e) if e.is_excluded() => {}
                    Err(e) => out.error(self.mode, &f.path, e.message()),
                },
            }
            out.touched += 1;
        }
    }

    // ------------------------------------------------------------ databases

    /// Database files a template resolves to (real files only, not excluded).
    fn databases(&self, tpl: &str, out: &mut Outcome) -> Vec<PathBuf> {
        let mut dbs = Vec::new();
        for f in template::resolve(&self.ctx.env, tpl) {
            let ft = f.meta.file_type();
            if ft.is_symlink() {
                out.error(self.mode, &f.path, "skipped: database is a symbolic link");
            } else if ft.is_file() && !self.safety.excludes.is_excluded(&f.path) {
                dbs.push(f.path);
            }
        }
        dbs
    }

    fn db_result(&self, db: &Path, r: std::result::Result<u64, DbError>, out: &mut Outcome) {
        match r {
            Ok(rows) => {
                out.rows += rows;
                out.touched += 1;
                if rows > 0 {
                    out.sample(db);
                }
            }
            Err(DbError::InUse) => {
                out.in_use += 1;
                out.error(self.mode, db, "in use");
            }
            Err(DbError::Other(m)) => out.error(self.mode, db, m),
        }
    }

    fn sqlite(&self, t: &SqliteTarget, out: &mut Outcome) {
        for db in self.databases(&t.db, out) {
            let r = match self.mode {
                Mode::Analyze => sqlite::count_rows(&db, &t.count_query),
                Mode::Clean => sqlite::clean_db(&db, &t.statements, &t.count_query).map(|o| o.rows),
            };
            self.db_result(&db, r, out);
        }
    }

    fn cookies(&self, t: &CookiesTarget, out: &mut Outcome) {
        let select = CookieSelect::AllExcept(&self.keep);
        for db in self.databases(&t.db, out) {
            let r = match self.mode {
                Mode::Analyze => sqlite::count_cookies(&db, t.browser_family, &select),
                Mode::Clean => {
                    sqlite::delete_cookies(&db, t.browser_family, &select).map(|o| o.rows)
                }
            };
            self.db_result(&db, r, out);
        }
    }

    // ------------------------------------------------------------ commands

    fn command(&self, t: &CommandTarget, out: &mut Outcome) {
        if self.ctx.runner.which(&t.program).is_none() {
            return;
        }
        let label = t.label.clone().unwrap_or_else(|| t.program.clone());
        match self.mode {
            Mode::Analyze => {
                out.actions.push(label);
                out.touched += 1;
            }
            Mode::Clean => {
                let args: Vec<&str> = t.args.iter().map(String::as_str).collect();
                match self.ctx.runner.run(&t.program, &args) {
                    Ok(o) if o.success() => {
                        out.actions.push(label);
                        out.touched += 1;
                    }
                    Ok(o) => {
                        let msg = o.stderr.trim();
                        let msg = if msg.is_empty() {
                            format!("exited with code {}", o.status)
                        } else {
                            msg.lines().next().unwrap_or(msg).to_string()
                        };
                        out.error(self.mode, Path::new(&label), msg);
                    }
                    Err(e) => out.error(self.mode, Path::new(&label), e.message),
                }
            }
        }
    }

    // ------------------------------------------------------------ trash

    fn trash(&self, job: &Job, out: &mut Outcome) -> Result<()> {
        let mk = |base: &str| FilesTarget {
            os: Vec::new(),
            base: base.to_string(),
            literal_base: None,
            pattern: "*".to_string(),
            recursive: true,
            min_age_hours: None,
            min_age_from_settings: false,
            remove_empty_dirs: true,
            keep_base: true,
            owned_by_user: false,
            skip_names: Vec::new(),
        };
        match self.ctx.env.os {
            Os::Linux => {
                // XDG trash: files/, info/ and expunged/ in the user's data dir.
                for sub in ["files", "info", "expunged"] {
                    self.files(&mk(&format!("{{data}}/Trash/{sub}")), job, out)?;
                }
            }
            Os::MacOs => self.files(&mk("{home}/.Trash"), job, out)?,
            Os::Windows => self.recycle_bin(out),
        }
        Ok(())
    }

    fn recycle_bin(&self, out: &mut Outcome) {
        const PS: &str = "powershell";
        if self.ctx.runner.which(PS).is_none() {
            out.unsupported += 1;
            return;
        }
        let size = self
            .ctx
            .runner
            .run(
                PS,
                &[
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    RECYCLE_SIZE_SCRIPT,
                ],
            )
            .ok()
            .filter(|o| o.success())
            .and_then(|o| parse_two_numbers(&o.stdout));
        let (files, bytes) = size.unwrap_or((0, 0));
        match self.mode {
            Mode::Analyze => {
                match size {
                    Some(_) => {
                        out.files += files;
                        out.bytes += bytes;
                        if files > 0 {
                            out.sample(Path::new("Recycle Bin"));
                        }
                    }
                    None => out
                        .actions
                        .push("Empty Recycle Bin (size unknown)".to_string()),
                }
                out.touched += 1;
            }
            Mode::Clean => {
                let r = self.ctx.runner.run(
                    PS,
                    &[
                        "-NoProfile",
                        "-NonInteractive",
                        "-Command",
                        RECYCLE_CLEAR_SCRIPT,
                    ],
                );
                match r {
                    Ok(o) if o.success() => {
                        out.files += files;
                        out.bytes += bytes;
                        out.actions.push("Empty Recycle Bin".to_string());
                        out.touched += 1;
                    }
                    Ok(o) => out.error(
                        self.mode,
                        Path::new("Recycle Bin"),
                        format!("exited with code {}", o.status),
                    ),
                    Err(e) => out.error(self.mode, Path::new("Recycle Bin"), e.message),
                }
            }
        }
    }

    // ------------------------------------------------------------ registry

    #[cfg(windows)]
    fn registry(&self, t: &RegistryTarget, out: &mut Outcome) {
        use crate::features::cleaner::registry_win as reg;
        if self.ctx.env.os != Os::Windows {
            out.unsupported += 1;
            return;
        }
        let r = match self.mode {
            Mode::Analyze => reg::count(&t.key, t.values, t.subkeys),
            Mode::Clean => reg::clear(&t.key, t.values, t.subkeys),
        };
        match r {
            Ok(n) => {
                out.rows += n;
                out.touched += 1;
                if n > 0 {
                    out.sample(Path::new(&t.key));
                }
            }
            Err(m) => out.error(self.mode, Path::new(&t.key), m),
        }
    }

    #[cfg(not(windows))]
    fn registry(&self, _t: &RegistryTarget, out: &mut Outcome) {
        out.unsupported += 1;
    }
}

// ---------------------------------------------------------------- helpers

fn compile_pattern(p: &str) -> GlobMatcher {
    GlobBuilder::new(p)
        .case_insensitive(CASE_INSENSITIVE)
        .build()
        .unwrap_or_else(|_| Glob::new("*").expect("static glob"))
        .compile_matcher()
}

/// Names never touched. An invalid entry would silently stop protecting something, so
/// it degrades to "match everything" (nothing gets cleaned) rather than to "match nothing".
fn compile_skip(names: &[String]) -> GlobSet {
    let mut b = GlobSetBuilder::new();
    for n in names {
        match GlobBuilder::new(n)
            .case_insensitive(CASE_INSENSITIVE)
            .build()
        {
            Ok(g) => {
                b.add(g);
            }
            Err(_) => {
                if let Ok(g) = Glob::new("*") {
                    b.add(g);
                }
            }
        }
    }
    b.build().unwrap_or_else(|_| GlobSet::empty())
}

/// Age and ownership filters.
fn filters_pass(
    meta: &std::fs::Metadata,
    min_age: Option<Duration>,
    owned_by_user: bool,
    now: SystemTime,
) -> bool {
    if owned_by_user && !owned_by_current_user(meta) {
        return false;
    }
    if let Some(age) = min_age {
        match meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
        {
            Some(d) if d >= age => {}
            // Younger, unreadable or future-dated mtime: keep it.
            _ => return false,
        }
    }
    true
}

#[cfg(unix)]
fn owned_by_current_user(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: geteuid has no preconditions and cannot fail.
    meta.uid() == unsafe { libc::geteuid() }
}

#[cfg(not(unix))]
fn owned_by_current_user(_meta: &std::fs::Metadata) -> bool {
    true
}

fn parse_two_numbers(s: &str) -> Option<(u64, u64)> {
    let mut it = s.split_whitespace();
    let a = it.next()?.parse().ok()?;
    let b = it.next()?.parse().ok()?;
    Some((a, b))
}
