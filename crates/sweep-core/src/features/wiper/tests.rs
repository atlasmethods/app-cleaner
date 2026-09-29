use super::drive::{DeviceIo, DeviceState};
use super::freespace::{self, disk_full_error, WipeFs, WIPE_PREFIX};
use super::*;
use crate::api::dispatch;
use crate::error::ErrorCode;
use crate::features::secure_delete::Pattern;
use crate::job::CancelToken;
use crate::runner::{CmdOutput, MockRunner};
use devices::{BlockDevice, Partition, Usage};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

const MIB: u64 = 1 << 20;

fn bed() -> (tempfile::TempDir, Ctx) {
    let d = tempfile::tempdir().unwrap();
    let ctx = Ctx::test(d.path(), MockRunner::new());
    fs::create_dir_all(&ctx.env.home).unwrap();
    fs::create_dir_all(&ctx.env.temp_dir).unwrap();
    (d, ctx)
}

fn entries(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

// ---------------------------------------------------------------- simulated small disk

/// A "disk" of `limit` bytes: growth beyond it fails with ENOSPC after a partial write.
struct LimitedFs {
    limit: u64,
    used: Mutex<u64>,
    calls: AtomicUsize,
    on_call: Box<dyn Fn(usize) -> io::Result<()> + Send + Sync>,
}

impl LimitedFs {
    fn new(limit: u64) -> Self {
        Self {
            limit,
            used: Mutex::new(0),
            calls: AtomicUsize::new(0),
            on_call: Box::new(|_| Ok(())),
        }
    }
    fn with_hook(mut self, f: impl Fn(usize) -> io::Result<()> + Send + Sync + 'static) -> Self {
        self.on_call = Box::new(f);
        self
    }
}

impl WipeFs for LimitedFs {
    fn free_space(&self, _dir: &Path) -> io::Result<u64> {
        Ok(self.limit - *self.used.lock().unwrap())
    }
    fn write(&self, file: &mut File, data: &[u8]) -> io::Result<()> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        (self.on_call)(n)?;
        let pos = file.stream_position()?;
        let len = file.metadata()?.len();
        let growth = (pos + data.len() as u64).saturating_sub(len);
        let mut used = self.used.lock().unwrap();
        let avail = self.limit - *used;
        if growth > avail {
            let allowed = data.len() - (growth - avail) as usize;
            file.write_all(&data[..allowed])?;
            *used += avail;
            return Err(disk_full_error());
        }
        *used += growth;
        file.write_all(data)
    }
}

fn run(
    ctx: &Ctx,
    mount: &Path,
    passes: u32,
    fs: &dyn WipeFs,
    job: &Job,
    max_file: u64,
    hook: Option<&dyn Fn(&freespace::PassDone<'_>)>,
) -> Result<freespace::FillReport> {
    run_free_space_wipe(ctx, mount, passes, fs, job, max_file, hook)
}

#[test]
fn fills_the_simulated_disk_and_removes_everything() {
    let (d, ctx) = bed();
    let mount = d.path().join("disk");
    fs::create_dir_all(&mount).unwrap();
    fs::write(mount.join("user-file.txt"), b"precious").unwrap();
    let limit = 5 * MIB + 300 * 1024;
    let fake = LimitedFs::new(limit);
    let r = run(&ctx, &mount, 1, &fake, &Job::detached(), 2 * MIB, None).unwrap();
    assert_eq!(r.files, 3, "2 MiB + 2 MiB + the remainder");
    assert_eq!(r.bytes_per_pass, limit);
    assert_eq!(r.bytes_written, limit);
    assert_eq!(r.free_before, Some(limit));
    assert_eq!(r.location, mount);
    assert_eq!(
        entries(&mount),
        vec!["user-file.txt"],
        "wipe folder is gone"
    );
    assert_eq!(fs::read(mount.join("user-file.txt")).unwrap(), b"precious");
}

#[test]
fn every_pass_writes_its_pattern_over_the_same_files() {
    let (d, ctx) = bed();
    let mount = d.path().join("disk");
    fs::create_dir_all(&mount).unwrap();
    let limit = 3 * MIB + 17;
    let fake = LimitedFs::new(limit);
    let seen: Mutex<Vec<(usize, Pattern, usize, u64)>> = Mutex::new(Vec::new());
    let check = |p: &freespace::PassDone<'_>| {
        assert!(p
            .dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(WIPE_PREFIX));
        let total: u64 = p.files.iter().map(|f| f.len).sum();
        assert_eq!(total, limit, "pass {} covers all the space", p.pass);
        for f in p.files {
            assert_eq!(fs::metadata(&f.path).unwrap().len(), f.len);
        }
        // read back the first and the last file and compare with the pattern
        for f in [p.files.first().unwrap(), p.files.last().unwrap()] {
            let mut buf = vec![0u8; f.len.min(65_536) as usize];
            let mut h = File::open(&f.path).unwrap();
            h.read_exact(&mut buf).unwrap();
            match p.pattern {
                Pattern::Byte(b) => assert!(buf.iter().all(|x| x == b), "pass {}", p.pass),
                Pattern::Random => {
                    assert!(buf.iter().any(|x| *x != buf[0]), "random data varies");
                }
                Pattern::Bytes(_) => unreachable!(),
            }
            // the tail of the file too
            let mut tail = vec![0u8; 4096.min(f.len as usize)];
            h.seek(SeekFrom::End(-(tail.len() as i64))).unwrap();
            h.read_exact(&mut tail).unwrap();
            if let Pattern::Byte(b) = p.pattern {
                assert!(tail.iter().all(|x| *x == *b));
            }
        }
        seen.lock()
            .unwrap()
            .push((p.pass, p.pattern.clone(), p.files.len(), total));
    };
    let r = run(&ctx, &mount, 3, &fake, &Job::detached(), MIB, Some(&check)).unwrap();
    let seen = seen.into_inner().unwrap();
    assert_eq!(seen.len(), 3);
    assert_eq!(seen[0].1, Pattern::Byte(0x00));
    assert_eq!(seen[1].1, Pattern::Byte(0xFF));
    assert_eq!(seen[2].1, Pattern::Random);
    assert!(seen.iter().all(|s| s.2 == 4), "same 4 files in every pass");
    assert_eq!(r.bytes_written, 3 * limit);
    assert!(entries(&mount).is_empty());
}

#[test]
fn seven_passes_use_the_secure_delete_schedule() {
    let (d, ctx) = bed();
    let mount = d.path().join("disk");
    fs::create_dir_all(&mount).unwrap();
    let fake = LimitedFs::new(MIB);
    let pats = Mutex::new(Vec::new());
    let hook = |p: &freespace::PassDone<'_>| pats.lock().unwrap().push(p.pattern.clone());
    run(&ctx, &mount, 7, &fake, &Job::detached(), MIB, Some(&hook)).unwrap();
    assert_eq!(pats.into_inner().unwrap(), overwrite_passes(7));
}

#[test]
fn cancelling_mid_fill_cleans_up() {
    let (d, ctx) = bed();
    let mount = d.path().join("disk");
    fs::create_dir_all(&mount).unwrap();
    let token = CancelToken::new();
    let t2 = token.clone();
    let fake = LimitedFs::new(20 * MIB).with_hook(move |n| {
        if n == 5 {
            t2.cancel();
        }
        Ok(())
    });
    let e = run(
        &ctx,
        &mount,
        3,
        &fake,
        &Job::with_token(token),
        2 * MIB,
        None,
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::Cancelled);
    assert!(entries(&mount).is_empty(), "{:?}", entries(&mount));
}

#[test]
fn cancelling_during_a_rewrite_pass_cleans_up() {
    let (d, ctx) = bed();
    let mount = d.path().join("disk");
    fs::create_dir_all(&mount).unwrap();
    let token = CancelToken::new();
    let t2 = token.clone();
    // 4 MiB disk, 1 MiB chunks: fill needs 5 write calls (4 + the failing one);
    // call 7 is inside the second pass.
    let fake = LimitedFs::new(4 * MIB).with_hook(move |n| {
        if n == 7 {
            t2.cancel();
        }
        Ok(())
    });
    let e = run(
        &ctx,
        &mount,
        3,
        &fake,
        &Job::with_token(token),
        2 * MIB,
        None,
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::Cancelled);
    assert!(entries(&mount).is_empty());
}

#[test]
fn a_write_error_other_than_disk_full_cleans_up_and_is_reported() {
    let (d, ctx) = bed();
    let mount = d.path().join("disk");
    fs::create_dir_all(&mount).unwrap();
    let fake = LimitedFs::new(20 * MIB).with_hook(|n| {
        if n == 3 {
            Err(io::Error::other("disk on fire"))
        } else {
            Ok(())
        }
    });
    let e = run(&ctx, &mount, 1, &fake, &Job::detached(), 2 * MIB, None).unwrap_err();
    assert!(e.message.contains("disk on fire"), "{e}");
    assert!(entries(&mount).is_empty());
}

#[test]
fn a_panic_still_removes_the_wipe_folder() {
    let (d, ctx) = bed();
    let mount = d.path().join("disk");
    fs::create_dir_all(&mount).unwrap();
    let fake = LimitedFs::new(3 * MIB);
    let hook = |_: &freespace::PassDone<'_>| panic!("boom");
    let r = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let _ = run(&ctx, &mount, 3, &fake, &Job::detached(), MIB, Some(&hook));
    }));
    assert!(r.is_err(), "the panic propagated");
    assert!(entries(&mount).is_empty(), "{:?}", entries(&mount));
}

#[test]
fn user_exclusions_cannot_keep_the_disk_full() {
    let (d, ctx) = bed();
    let mount = d.path().join("disk");
    fs::create_dir_all(&mount).unwrap();
    crate::features::settings::update(&ctx, |s| {
        s.exclude.push(crate::features::settings::ExcludeEntry {
            id: "e".into(),
            pattern: mount.to_string_lossy().into_owned(),
        });
    })
    .unwrap();
    let fake = LimitedFs::new(2 * MIB);
    run(&ctx, &mount, 1, &fake, &Job::detached(), MIB, None).unwrap();
    assert!(entries(&mount).is_empty());
}

#[test]
fn cleanup_works_even_inside_a_protected_folder() {
    let (_d, ctx) = bed();
    // ~/.ssh is protected; SafeDeleter refuses to touch it, the fallback must not.
    let mount = ctx.env.home.join(".ssh").join("mnt");
    fs::create_dir_all(&mount).unwrap();
    let fake = LimitedFs::new(2 * MIB);
    run(&ctx, &mount, 1, &fake, &Job::detached(), MIB, None).unwrap();
    assert!(entries(&mount).is_empty());
}

#[test]
fn stale_wipe_folders_are_removed_at_the_start_and_lookalikes_are_not() {
    let (d, ctx) = bed();
    let mount = d.path().join("disk");
    fs::create_dir_all(&mount).unwrap();
    let stale = mount.join(format!("{WIPE_PREFIX}0123456789abcdef"));
    fs::create_dir_all(&stale).unwrap();
    fs::write(stale.join("fill-000000.bin"), vec![1u8; 4096]).unwrap();
    fs::write(stale.join("fill-000001.bin"), vec![1u8; 4096]).unwrap();
    // things that merely look similar
    let user_dir = mount.join(format!("{WIPE_PREFIX}mine"));
    fs::create_dir_all(&user_dir).unwrap();
    fs::write(user_dir.join("keep.txt"), b"mine").unwrap();
    let short = mount.join(format!("{WIPE_PREFIX}0123"));
    fs::create_dir_all(&short).unwrap();
    let a_file = mount.join(format!("{WIPE_PREFIX}fedcba9876543210"));
    fs::write(&a_file, b"a plain file").unwrap();
    let with_subdir = mount.join(format!("{WIPE_PREFIX}aaaaaaaaaaaaaaaa"));
    fs::create_dir_all(with_subdir.join("nested")).unwrap();
    fs::write(with_subdir.join("nested/user.txt"), b"user data").unwrap();

    let fake = LimitedFs::new(MIB);
    let r = run(&ctx, &mount, 1, &fake, &Job::detached(), MIB, None).unwrap();
    assert_eq!(r.stale_removed, 1);
    assert!(!stale.exists());
    assert!(user_dir.join("keep.txt").exists());
    assert!(short.exists());
    assert_eq!(fs::read(&a_file).unwrap(), b"a plain file");
    assert!(
        with_subdir.join("nested/user.txt").exists(),
        "never recurses"
    );
}

#[test]
fn a_running_wipe_is_never_mistaken_for_a_stale_one() {
    let (d, ctx) = bed();
    let mount = d.path().join("disk");
    fs::create_dir_all(&mount).unwrap();
    let safety = Arc::new(Safety::new(&ctx.env, ExcludeSet::empty()));
    // A live guard holds its lock: a second run must leave its folder alone ...
    let mut live = freespace::WipeGuard::create(&[mount.clone()], &mount, &safety).unwrap();
    assert!(freespace::remove_stale(&mount, &safety).is_empty());
    assert!(live.dir().exists());
    // ... including a whole second wipe of the same volume running at the same time.
    let fake = LimitedFs::new(MIB);
    let r = run(&ctx, &mount, 1, &fake, &Job::detached(), MIB, None).unwrap();
    assert_eq!(r.stale_removed, 0);
    assert!(live.dir().exists(), "the other run's folder survived");
    // Once the owner is done (or dead: the OS drops the lock), the folder is stale.
    let dir = live.dir().to_path_buf();
    live.cleanup().unwrap();
    assert!(!dir.exists());
    let crashed = mount.join(format!("{WIPE_PREFIX}00000000000000aa"));
    fs::create_dir_all(&crashed).unwrap();
    fs::write(crashed.join(".owner"), b"").unwrap(); // lock released: nobody holds it
    fs::write(crashed.join("fill-000000.bin"), vec![0u8; 1024]).unwrap();
    assert_eq!(
        freespace::remove_stale(&mount, &safety),
        vec![crashed.clone()]
    );
    assert!(!crashed.exists());
}

#[test]
fn an_unwritable_location_is_permission_denied() {
    let (d, ctx) = bed();
    let mount = d.path().join("does-not-exist");
    let fake = LimitedFs::new(MIB);
    let e = run(&ctx, &mount, 1, &fake, &Job::detached(), MIB, None).unwrap_err();
    assert_eq!(e.code, ErrorCode::PermissionDenied);
}

#[test]
fn falls_back_to_home_or_temp_on_the_same_volume() {
    let (d, ctx) = bed();
    let mount = d.path().to_path_buf();
    // The mount itself is read-only for the purposes of this test: a plain file where
    // the wipe folder would go is not the point; instead give an explicit candidate list.
    let blocked = d.path().join("blocked");
    fs::write(&blocked, b"a file, not a folder").unwrap();
    let fake = LimitedFs::new(MIB);
    let safety = Arc::new(Safety::new(&ctx.env, ExcludeSet::empty()));
    let r = fill_free_space(&FillParams {
        mount: &mount,
        safety,
        candidates: vec![blocked.clone(), ctx.env.home.clone()],
        patterns: vec![Pattern::Byte(0)],
        fs: &fake,
        job: &Job::detached(),
        max_file: MIB,
        on_pass_done: None,
    })
    .unwrap();
    assert_eq!(r.location, ctx.env.home);
    assert!(entries(&ctx.env.home).is_empty());
    let cands = freespace::candidate_locations(&ctx, &mount);
    assert_eq!(cands[0], mount);
    assert!(cands.contains(&ctx.env.home));
}

#[test]
fn progress_is_reported_and_bounded() {
    let (d, ctx) = bed();
    let mount = d.path().join("disk");
    fs::create_dir_all(&mount).unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let ev = events.clone();
    let job = Job::new(CancelToken::new(), move |e| ev.lock().unwrap().push(e));
    let fake = LimitedFs::new(2 * MIB);
    run(&ctx, &mount, 3, &fake, &job, MIB, None).unwrap();
    let events = events.lock().unwrap();
    assert!(events.len() >= 3, "at least one event per pass");
    for e in events.iter() {
        assert_eq!(e.stage, "wipe");
        assert!(e.fraction.unwrap() >= 0.0 && e.fraction.unwrap() <= 1.0);
    }
    assert!(events[0]
        .message
        .as_deref()
        .unwrap()
        .contains("Pass 1 of 3"));
}

#[test]
fn api_validates_passes_and_mount() {
    let (_d, ctx) = bed();
    for (params, code) in [
        (json!({"mount": "/", "passes": 2}), ErrorCode::InvalidParams),
        (
            json!({"mount": "/definitely/not/a/mount", "passes": 1}),
            ErrorCode::InvalidParams,
        ),
        (
            json!({"mount": ctx.env.home, "passes": 1}),
            ErrorCode::InvalidParams,
        ),
        (json!({"passes": 1}), ErrorCode::InvalidParams),
    ] {
        let e = dispatch(
            &ctx,
            "wiper.wipe_free_space",
            params.clone(),
            &Job::detached(),
        )
        .unwrap_err();
        assert_eq!(e.code, code, "{params}");
    }
}

#[test]
fn real_free_space_query_works() {
    let d = tempfile::tempdir().unwrap();
    let free = freespace::RealFs.free_space(d.path()).unwrap();
    assert!(free > 0);
    assert!(freespace::RealFs
        .free_space(&d.path().join("nope"))
        .is_err());
    assert!(freespace::same_volume(d.path(), d.path()));
    assert!(!freespace::same_volume(d.path(), &d.path().join("nope")));
}

#[test]
fn disk_full_detection() {
    assert!(freespace::is_disk_full(&disk_full_error()));
    assert!(!freespace::is_disk_full(&io::Error::other("x")));
    assert!(!freespace::is_disk_full(&io::Error::from(
        io::ErrorKind::NotFound
    )));
}

// ---------------------------------------------------------------- real tmpfs

/// A real size-capped tmpfs mounted on a temp dir (root only). `None` when the mount is
/// not permitted, so callers can skip.
pub(crate) struct Tmpfs {
    dir: PathBuf,
    _guard: Option<tempfile::TempDir>,
}

impl Tmpfs {
    /// Mount on a fresh temp dir.
    pub(crate) fn mount(size_mb: u32) -> Option<Tmpfs> {
        let guard = tempfile::tempdir().ok()?;
        let mut t = Self::mount_at(guard.path(), size_mb)?;
        t._guard = Some(guard);
        Some(t)
    }

    /// Mount on an existing directory.
    pub(crate) fn mount_at(dir: &Path, size_mb: u32) -> Option<Tmpfs> {
        #[cfg(target_os = "linux")]
        {
            let ok = std::process::Command::new("mount")
                .args(["-t", "tmpfs", "-o", &format!("size={size_mb}m"), "tmpfs"])
                .arg(dir)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            ok.then(|| Tmpfs {
                dir: dir.to_path_buf(),
                _guard: None,
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (dir, size_mb);
            None
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Tmpfs {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        {
            let _ = std::process::Command::new("umount").arg(&self.dir).status();
        }
    }
}

/// Real ENOSPC on a 32 MiB tmpfs: run with `cargo test -p sweep-core -- --ignored tmpfs`
/// as root (scripts/check.sh does).
#[test]
#[ignore = "needs root and the ability to mount a tmpfs"]
fn tmpfs_free_space_wipe_end_to_end() {
    let Some(mnt) = Tmpfs::mount(32) else {
        eprintln!("SKIPPED: cannot mount a tmpfs here");
        return;
    };
    let (_d, ctx) = bed();
    let mount = mnt.path().to_path_buf();
    fs::write(mount.join("user-file.txt"), b"keep me").unwrap();
    let free0 = freespace::RealFs.free_space(&mount).unwrap();
    assert!(free0 > 20 * MIB && free0 < 33 * MIB, "{free0}");

    let checked = AtomicUsize::new(0);
    let hook = |p: &freespace::PassDone<'_>| {
        // The disk is really full at this point.
        let free = freespace::RealFs.free_space(p.dir).unwrap();
        assert!(free < 64 * 1024, "pass {}: {free} bytes still free", p.pass);
        let f = &p.files[0];
        let mut buf = vec![0u8; 8192];
        File::open(&f.path).unwrap().read_exact(&mut buf).unwrap();
        match p.pattern {
            Pattern::Byte(b) => assert!(buf.iter().all(|x| x == b)),
            _ => assert!(buf.iter().any(|x| *x != buf[0])),
        }
        checked.fetch_add(1, Ordering::SeqCst);
    };
    let r = run_free_space_wipe(
        &ctx,
        &mount,
        3,
        &RealFs,
        &Job::detached(),
        8 * MIB,
        Some(&hook),
    )
    .unwrap();
    assert_eq!(checked.load(Ordering::SeqCst), 3);
    assert!(r.files >= 3);
    assert!(
        r.bytes_per_pass >= free0 - 64 * 1024,
        "{} vs {free0}",
        r.bytes_per_pass
    );

    // afterwards: everything back, nothing left
    assert_eq!(entries(&mount), vec!["user-file.txt"]);
    let free1 = freespace::RealFs.free_space(&mount).unwrap();
    assert!(
        free1 + 8192 >= free0,
        "free space restored: {free1} vs {free0}"
    );
    assert_eq!(fs::read(mount.join("user-file.txt")).unwrap(), b"keep me");
}

/// A 16 MiB ext4 image on a loop device (root only): a real journaling filesystem with
/// metadata and reserved blocks. `None` when loop mounts or `mkfs.ext4` are unavailable.
struct Ext4Loop {
    mount: PathBuf,
    _dir: tempfile::TempDir,
}

impl Ext4Loop {
    fn new() -> Option<Ext4Loop> {
        let dir = tempfile::tempdir().ok()?;
        let img = dir.path().join("fs.img");
        let mount = dir.path().join("mnt");
        fs::create_dir_all(&mount).ok()?;
        File::create(&img).ok()?.set_len(16 * MIB).ok()?;
        let ok = |mut c: std::process::Command| c.status().map(|s| s.success()).unwrap_or(false);
        let mut mk = std::process::Command::new("mkfs.ext4");
        mk.args(["-q", "-F"]).arg(&img);
        let mut mt = std::process::Command::new("mount");
        mt.args(["-o", "loop"]).arg(&img).arg(&mount);
        (ok(mk) && ok(mt)).then_some(Ext4Loop { mount, _dir: dir })
    }
}

impl Drop for Ext4Loop {
    fn drop(&mut self) {
        let _ = std::process::Command::new("umount")
            .arg(&self.mount)
            .status();
    }
}

#[test]
#[ignore = "needs root, loop devices and mkfs.ext4"]
fn ext4_free_space_wipe_end_to_end() {
    let Some(fsys) = Ext4Loop::new() else {
        eprintln!("SKIPPED: cannot create an ext4 loop mount here");
        return;
    };
    let (_d, ctx) = bed();
    let mount = fsys.mount.clone();
    fs::write(mount.join("keep.txt"), b"keep me").unwrap();
    let free0 = freespace::RealFs.free_space(&mount).unwrap();
    let hook = |p: &freespace::PassDone<'_>| {
        let mut buf = vec![0u8; 4096];
        File::open(&p.files[0].path)
            .unwrap()
            .read_exact(&mut buf)
            .unwrap();
        if let Pattern::Byte(b) = p.pattern {
            assert!(buf.iter().all(|x| x == b), "pass {}", p.pass);
        }
    };
    let r = run_free_space_wipe(
        &ctx,
        &mount,
        3,
        &RealFs,
        &Job::detached(),
        4 * MIB,
        Some(&hook),
    )
    .unwrap();
    assert!(r.files >= 2);
    assert!(
        r.bytes_per_pass >= free0 / 2,
        "{} vs {free0}",
        r.bytes_per_pass
    );
    let left: Vec<String> = entries(&mount)
        .into_iter()
        .filter(|n| n != "lost+found")
        .collect();
    assert_eq!(left, vec!["keep.txt"]);
    let free1 = freespace::RealFs.free_space(&mount).unwrap();
    assert!(free1 + 64 * 1024 >= free0, "restored: {free1} vs {free0}");
    assert_eq!(fs::read(mount.join("keep.txt")).unwrap(), b"keep me");
}

#[test]
#[ignore = "needs root and the ability to mount a tmpfs"]
fn tmpfs_cancel_and_stale_recovery() {
    let Some(mnt) = Tmpfs::mount(16) else {
        eprintln!("SKIPPED: cannot mount a tmpfs here");
        return;
    };
    let (_d, ctx) = bed();
    let mount = mnt.path().to_path_buf();
    let free0 = freespace::RealFs.free_space(&mount).unwrap();

    // cancel while the disk is being filled
    let token = CancelToken::new();
    let t2 = token.clone();
    let job = Job::new(token, move |e| {
        if e.current.unwrap_or(0) > 4 * MIB {
            t2.cancel();
        }
    });
    let e = run_free_space_wipe(&ctx, &mount, 1, &RealFs, &job, MIB, None).unwrap_err();
    assert_eq!(e.code, ErrorCode::Cancelled);
    assert!(entries(&mount).is_empty());
    assert!(freespace::RealFs.free_space(&mount).unwrap() + 8192 >= free0);

    // a crashed run: a stale folder that fills the disk; the next run cleans it first
    let stale = mount.join(format!("{WIPE_PREFIX}0011223344556677"));
    fs::create_dir_all(&stale).unwrap();
    let mut f = File::create(stale.join("fill-000000.bin")).unwrap();
    let chunk = vec![0u8; MIB as usize];
    while f.write_all(&chunk).is_ok() {}
    drop(f);
    assert!(
        freespace::RealFs.free_space(&mount).unwrap() < 64 * 1024,
        "disk is full"
    );
    let r = run_free_space_wipe(&ctx, &mount, 1, &RealFs, &Job::detached(), 4 * MIB, None).unwrap();
    assert_eq!(r.stale_removed, 1);
    assert!(
        r.bytes_per_pass > 10 * MIB,
        "the space freed by removing the stale folder was wiped"
    );
    assert!(entries(&mount).is_empty());
}

// ---------------------------------------------------------------- devices (fake sysfs)

fn write_file(p: &Path, s: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, s).unwrap();
}

/// sda (system: partitions mounted at / and /boot/efi), sdb (USB stick: sdb1 mounted),
/// sdc (idle), sdd (idle, sdd1 is an LVM PV with a holder), sde (idle, swap on sde1),
/// plus loop0 / zram0 which must not be listed.
fn fake_machine(ctx: &Ctx) {
    let root = &ctx.env.root;
    let dev = |n: &str| {
        ctx.env
            .sys_path(format!("/dev/{n}"))
            .to_string_lossy()
            .into_owned()
    };
    for (name, sectors, removable, model, parts) in [
        ("sda", 1_000_000u64, "0", "System SSD", vec!["sda1", "sda2"]),
        ("sdb", 4_000_000, "1", "USB Stick", vec!["sdb1"]),
        ("sdc", 2_000_000, "0", "Spare Disk", vec![]),
        ("sdd", 2_000_000, "0", "LVM Disk", vec!["sdd1"]),
        ("sde", 2_000_000, "0", "Swap Disk", vec!["sde1"]),
    ] {
        let d = root.join("sys/block").join(name);
        write_file(&d.join("size"), &format!("{sectors}\n"));
        write_file(&d.join("removable"), &format!("{removable}\n"));
        write_file(&d.join("device/model"), &format!("{model}  \n"));
        for p in parts {
            write_file(&d.join(p).join("partition"), "1\n");
            write_file(&d.join(p).join("size"), "1000\n");
        }
    }
    fs::create_dir_all(root.join("sys/block/sdd/sdd1/holders/dm-0")).unwrap();
    fs::create_dir_all(root.join("sys/block/loop0")).unwrap();
    fs::create_dir_all(root.join("sys/block/zram0")).unwrap();
    write_file(
        &root.join("proc/mounts"),
        &format!(
            "{} / ext4 rw 0 0\n{} /boot/efi vfat rw 0 0\n{} /media/user/USB\\040STICK vfat rw 0 0\nproc /proc proc rw 0 0\n",
            dev("sda2"),
            dev("sda1"),
            dev("sdb1")
        ),
    );
    write_file(
        &root.join("proc/swaps"),
        &format!(
            "Filename\tType\tSize\tUsed\tPriority\n{} partition 1000 0 -2\n",
            dev("sde1")
        ),
    );
}

#[test]
fn block_devices_are_enumerated_from_sysfs() {
    let (_d, ctx) = bed();
    fake_machine(&ctx);
    let devs = devices::list_block_devices(&ctx.env);
    let names: Vec<&str> = devs.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["sda", "sdb", "sdc", "sdd", "sde"],
        "loop / zram are not listed"
    );
    let sdb = &devs[1];
    assert_eq!(sdb.size_bytes, 4_000_000 * 512);
    assert!(sdb.removable);
    assert_eq!(sdb.model.as_deref(), Some("USB Stick"));
    assert_eq!(sdb.partitions.len(), 1);
    assert_eq!(sdb.partitions[0].size_bytes, 1000 * 512);
    assert!(sdb.device.ends_with("/dev/sdb"));

    let mounts = devices::read_mounts(&ctx.env);
    let swaps = devices::read_swaps(&ctx.env);
    let usage: Vec<Usage> = devs
        .iter()
        .map(|d| devices::usage_of(d, &mounts, &[], &swaps))
        .collect();
    assert_eq!(usage[0].mounts.len(), 2);
    assert_eq!(usage[1].mounts, vec!["/media/user/USB STICK"]);
    assert!(!usage[2].in_use());
    assert_eq!(usage[3].holders, vec!["dm-0"]);
    assert!(usage[3].in_use());
    assert!(usage[4].swap && usage[4].in_use());

    let by_part = devices::whole_disk_of(&devs, &devs[1].partitions[0].device);
    assert_eq!(by_part.as_deref(), Some(devs[1].device.as_str()));
    assert_eq!(devices::whole_disk_of(&devs, "/dev/nothing"), None);
}

#[test]
fn list_devices_api_flags_system_and_in_use_disks() {
    let (_d, ctx) = bed();
    fake_machine(&ctx);
    let v = dispatch(&ctx, "wiper.list_devices", json!({}), &Job::detached()).unwrap();
    assert_eq!(v["supported"], true);
    let d = v["devices"].as_array().unwrap();
    assert_eq!(d.len(), 5);
    assert_eq!(d[0]["isSystem"], true);
    assert_eq!(d[1]["isSystem"], false);
    assert_eq!(d[1]["mounts"][0], "/media/user/USB STICK");
    assert_eq!(d[2]["mounts"].as_array().unwrap().len(), 0);
    assert_eq!(d[4]["swap"], true);
    assert_eq!(d[1]["removable"], true);
    // other platforms report "not supported" rather than guessing
    let mut win = ctx.clone();
    win.env.os = Os::Windows;
    let v = dispatch(&win, "wiper.list_devices", json!({}), &Job::detached()).unwrap();
    assert_eq!(v["supported"], false);
}

#[test]
fn a_root_filesystem_reported_as_dev_root_is_recognised_by_device_number() {
    // Raspberry-Pi style: /proc/mounts says `/dev/root`, which is no device node we know.
    let (_d, ctx) = bed();
    let root = &ctx.env.root;
    let sys = root.join("sys/block/mmcblk0");
    write_file(&sys.join("size"), "1000000\n");
    write_file(&sys.join("dev"), "179:0\n");
    write_file(&sys.join("mmcblk0p1/partition"), "1\n");
    write_file(&sys.join("mmcblk0p1/dev"), "179:1\n");
    write_file(&sys.join("mmcblk0p2/partition"), "2\n");
    write_file(&sys.join("mmcblk0p2/dev"), "179:2\n");
    write_file(
        &root.join("proc/mounts"),
        "/dev/root / ext4 rw 0 0\nproc /proc proc rw 0 0\n",
    );
    write_file(
        &root.join("proc/self/mountinfo"),
        "20 1 179:2 / / rw,relatime - ext4 /dev/root rw\n21 20 0:5 / /proc rw - proc proc rw\n",
    );
    let d = ctx
        .env
        .sys_path("/dev/mmcblk0")
        .to_string_lossy()
        .into_owned();
    let listing = dispatch(&ctx, "wiper.list_devices", json!({}), &Job::detached()).unwrap();
    assert_eq!(listing["devices"][0]["isSystem"], true, "{listing}");
    assert_eq!(listing["devices"][0]["mounts"][0], "/");
    let e = dispatch(
        &ctx,
        "wiper.wipe_drive",
        json!({"device": d, "passes": 1, "confirm": d}),
        &Job::detached(),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::PermissionDenied);
    assert!(e.message.contains("operating system"), "{e}");
}

#[test]
fn a_drive_is_refused_when_the_mount_list_cannot_be_read() {
    let (_d, ctx) = bed();
    let sys = ctx.env.root.join("sys/block/sdz");
    write_file(&sys.join("size"), "1000\n");
    let d = ctx.env.sys_path("/dev/sdz").to_string_lossy().into_owned();
    let listing = dispatch(&ctx, "wiper.list_devices", json!({}), &Job::detached()).unwrap();
    assert_eq!(listing["devices"][0]["mountsKnown"], false);
    let e = dispatch(
        &ctx,
        "wiper.wipe_drive",
        json!({"device": d, "passes": 1, "confirm": d}),
        &Job::detached(),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::PermissionDenied);
    assert!(e.message.contains("cannot read the list of mounted"), "{e}");
}

// ---------------------------------------------------------------- wipe_drive

struct FakeBackend {
    devices: Vec<DeviceState>,
    elevated: bool,
    file: PathBuf,
    opened: AtomicUsize,
    synced: Arc<Mutex<Vec<u8>>>,
}

struct RecordingFile {
    f: File,
    path: PathBuf,
    first_bytes: Arc<Mutex<Vec<u8>>>,
}

impl Write for RecordingFile {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.f.write(b)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.f.flush()
    }
}
impl Seek for RecordingFile {
    fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
        self.f.seek(p)
    }
}
impl DeviceIo for RecordingFile {
    fn sync(&mut self) -> io::Result<()> {
        self.f.sync_all()?;
        let mut b = [0u8; 1];
        File::open(&self.path)?.read_exact(&mut b)?;
        self.first_bytes.lock().unwrap().push(b[0]);
        Ok(())
    }
}

impl drive::DriveBackend for FakeBackend {
    fn devices(&self, _ctx: &Ctx) -> Vec<DeviceState> {
        self.devices.clone()
    }
    fn is_elevated(&self) -> bool {
        self.elevated
    }
    fn open(&self, _device: &str) -> io::Result<Box<dyn DeviceIo>> {
        self.opened.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(RecordingFile {
            f: OpenOptions::new().write(true).open(&self.file)?,
            path: self.file.clone(),
            first_bytes: self.synced.clone(),
        }))
    }
}

fn state(dev: &str, usage: Usage, is_system: bool) -> DeviceState {
    DeviceState {
        device: BlockDevice {
            majmin: None,
            name: dev.trim_start_matches("/dev/").into(),
            device: dev.into(),
            size_bytes: 1 << 30,
            removable: true,
            model: None,
            partitions: vec![Partition {
                majmin: None,
                name: "p1".into(),
                device: format!("{dev}1"),
                size_bytes: 1 << 20,
                holders: vec![],
            }],
            holders: vec![],
        },
        usage,
        is_system,
        mounts_known: true,
    }
}

struct DriveBed {
    _d: tempfile::TempDir,
    ctx: Ctx,
    backend: FakeBackend,
}

const DISK_LEN: usize = 3 * (1 << 20) + 123;

fn drive_bed(elevated: bool) -> DriveBed {
    let (d, ctx) = bed();
    let file = d.path().join("fake-disk.img");
    fs::write(&file, vec![0xAAu8; DISK_LEN]).unwrap();
    let backend = FakeBackend {
        devices: vec![
            state("/dev/sdx", Usage::default(), false),
            state(
                "/dev/sdsys",
                Usage {
                    mounts: vec!["/".into()],
                    ..Default::default()
                },
                true,
            ),
            state(
                "/dev/sdmnt",
                Usage {
                    mounts: vec!["/media/usb".into(), "/mnt/b".into()],
                    ..Default::default()
                },
                false,
            ),
            state(
                "/dev/sdswap",
                Usage {
                    swap: true,
                    ..Default::default()
                },
                false,
            ),
            state(
                "/dev/sdlvm",
                Usage {
                    holders: vec!["dm-3".into()],
                    ..Default::default()
                },
                false,
            ),
        ],
        elevated,
        file,
        opened: AtomicUsize::new(0),
        synced: Arc::new(Mutex::new(Vec::new())),
    };
    DriveBed {
        _d: d,
        ctx,
        backend,
    }
}

fn wipe(b: &DriveBed, params: Value) -> Result<Value> {
    drive::wipe_drive(&b.ctx, params, &Job::detached(), &b.backend)
}

fn untouched(b: &DriveBed) {
    assert_eq!(
        b.backend.opened.load(Ordering::SeqCst),
        0,
        "device was opened"
    );
    let data = fs::read(&b.backend.file).unwrap();
    assert_eq!(data.len(), DISK_LEN);
    assert!(
        data.iter().all(|x| *x == 0xAA),
        "device content was modified"
    );
}

#[test]
fn wipe_drive_overwrites_the_whole_device_with_each_pass() {
    let b = drive_bed(true);
    let r = wipe(
        &b,
        json!({"device": "/dev/sdx", "passes": 3, "confirm": "/dev/sdx"}),
    )
    .unwrap();
    assert_eq!(r["bytes"], DISK_LEN);
    let data = fs::read(&b.backend.file).unwrap();
    assert_eq!(data.len(), DISK_LEN, "the device size is not changed");
    assert!(
        data.iter().any(|x| *x != data[0]),
        "last pass is random data"
    );
    assert_eq!(
        b.backend.synced.lock().unwrap().len(),
        3,
        "synced after every pass"
    );
    assert_eq!(b.backend.synced.lock().unwrap()[0], 0x00);
    assert_eq!(b.backend.synced.lock().unwrap()[1], 0xFF);
}

#[test]
fn single_pass_is_random_and_covers_every_byte() {
    let b = drive_bed(true);
    wipe(
        &b,
        json!({"device": "/dev/sdx", "passes": 1, "confirm": "/dev/sdx"}),
    )
    .unwrap();
    let data = fs::read(&b.backend.file).unwrap();
    // 0xAA can occur by chance, but a whole 4 KiB block of it cannot
    for chunk in data.chunks(4096) {
        assert!(chunk.iter().any(|x| *x != 0xAA));
    }
}

#[test]
fn wipe_drive_refusals_never_touch_the_device() {
    let b = drive_bed(true);
    let cases: Vec<(Value, ErrorCode, &str)> = vec![
        (
            json!({"device": "/dev/sdx", "passes": 1, "confirm": ""}),
            ErrorCode::InvalidParams,
            "confirmation",
        ),
        (
            json!({"device": "/dev/sdx", "passes": 1}),
            ErrorCode::InvalidParams,
            "confirmation",
        ),
        (
            json!({"device": "/dev/sdx", "passes": 1, "confirm": "/dev/sdx "}),
            ErrorCode::InvalidParams,
            "confirmation",
        ),
        (
            json!({"device": "/dev/sdx", "passes": 1, "confirm": "sdx"}),
            ErrorCode::InvalidParams,
            "confirmation",
        ),
        (
            json!({"device": "/dev/sdx", "passes": 1, "confirm": "/dev/SDX"}),
            ErrorCode::InvalidParams,
            "confirmation",
        ),
        (
            json!({"device": "/dev/sdx", "passes": 2, "confirm": "/dev/sdx"}),
            ErrorCode::InvalidParams,
            "passes",
        ),
        (
            json!({"device": "/dev/nope", "passes": 1, "confirm": "/dev/nope"}),
            ErrorCode::NotFound,
            "whole physical disk",
        ),
        (
            json!({"device": "/dev/sdx1", "passes": 1, "confirm": "/dev/sdx1"}),
            ErrorCode::NotFound,
            "whole physical disk",
        ),
        (
            json!({"device": "/dev/sdsys", "passes": 1, "confirm": "/dev/sdsys"}),
            ErrorCode::PermissionDenied,
            "operating system",
        ),
        (
            json!({"device": "/dev/sdmnt", "passes": 1, "confirm": "/dev/sdmnt"}),
            ErrorCode::PermissionDenied,
            "/media/usb, /mnt/b",
        ),
        (
            json!({"device": "/dev/sdswap", "passes": 1, "confirm": "/dev/sdswap"}),
            ErrorCode::PermissionDenied,
            "swap",
        ),
        (
            json!({"device": "/dev/sdlvm", "passes": 1, "confirm": "/dev/sdlvm"}),
            ErrorCode::PermissionDenied,
            "dm-3",
        ),
    ];
    for (params, code, needle) in cases {
        let e = wipe(&b, params.clone()).unwrap_err();
        assert_eq!(e.code, code, "{params}: {e}");
        assert!(e.message.contains(needle), "{params}: {}", e.message);
    }
    untouched(&b);
}

#[test]
fn wipe_drive_requires_administrator_rights() {
    let b = drive_bed(false);
    let e = wipe(
        &b,
        json!({"device": "/dev/sdx", "passes": 1, "confirm": "/dev/sdx"}),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::PermissionDenied);
    assert!(
        e.message.contains("Administrator rights are required"),
        "{e}"
    );
    untouched(&b);
}

#[test]
fn system_disk_is_refused_even_when_elevated_and_confirmed() {
    let b = drive_bed(true);
    let e = wipe(
        &b,
        json!({"device": "/dev/sdsys", "passes": 35, "confirm": "/dev/sdsys"}),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::PermissionDenied);
    untouched(&b);
}

#[test]
fn wipe_drive_can_be_cancelled_midway() {
    let b = drive_bed(true);
    let token = CancelToken::new();
    token.cancel();
    let e = drive::wipe_drive(
        &b.ctx,
        json!({"device": "/dev/sdx", "passes": 1, "confirm": "/dev/sdx"}),
        &Job::with_token(token),
        &b.backend,
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::Cancelled);
}

#[test]
fn windows_whole_drive_wipe_is_unsupported() {
    let mut b = drive_bed(true);
    b.ctx.env.os = Os::Windows;
    let e = wipe(
        &b,
        json!({"device": "/dev/sdx", "passes": 1, "confirm": "/dev/sdx"}),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::Unsupported);
    assert!(e.message.contains("not supported on Windows"));
    untouched(&b);
}

#[test]
fn real_backend_refuses_the_fake_system_disk_and_mounted_disks() {
    let (_d, ctx) = bed();
    fake_machine(&ctx);
    let dev = |n: &str| {
        ctx.env
            .sys_path(format!("/dev/{n}"))
            .to_string_lossy()
            .into_owned()
    };
    for (name, needle) in [
        ("sda", "operating system"),
        ("sdb", "/media/user/USB STICK"),
        ("sde", "swap"),
        ("sdd", "dm-0"),
    ] {
        let d = dev(name);
        let e = dispatch(
            &ctx,
            "wiper.wipe_drive",
            json!({"device": d, "passes": 1, "confirm": d}),
            &Job::detached(),
        )
        .unwrap_err();
        assert_eq!(e.code, ErrorCode::PermissionDenied, "{name}: {e}");
        assert!(e.message.contains(needle), "{name}: {e}");
    }
    // wrong confirm on an idle disk
    let d = dev("sdc");
    let e = dispatch(
        &ctx,
        "wiper.wipe_drive",
        json!({"device": d, "passes": 1, "confirm": "sdc"}),
        &Job::detached(),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidParams);
    // an idle disk that does not exist as a node: reaches the open step and fails there
    // without writing anything (or is refused for lack of privileges when not root)
    let e = dispatch(
        &ctx,
        "wiper.wipe_drive",
        json!({"device": d, "passes": 1, "confirm": d}),
        &Job::detached(),
    )
    .unwrap_err();
    assert!(
        matches!(e.code, ErrorCode::PermissionDenied | ErrorCode::NotFound),
        "{e}"
    );
    assert!(!Path::new(&d).exists());
}

// ---------------------------------------------------------------- macOS

fn mac_ctx(runner: &MockRunner) -> Ctx {
    let d = tempfile::tempdir().unwrap();
    let mut ctx = Ctx::test(d.path(), runner.clone());
    std::mem::forget(d);
    ctx.env.os = Os::MacOs;
    ctx
}

const BOOT_INFO: &str = "   Device Identifier:         disk3s1s1\n   Part of Whole:             disk3\n   APFS Physical Store:       disk0s2\n";
const EXT_INFO: &str = "   Device Identifier:         disk5\n   Whole:                     Yes\n   Removable Media:           Yes\n";

fn mac_wipe(runner: &MockRunner, elevated: bool, params: Value) -> Result<Value> {
    let ctx = mac_ctx(runner);
    let b = FakeBackend {
        devices: vec![],
        elevated,
        file: PathBuf::new(),
        opened: AtomicUsize::new(0),
        synced: Arc::default(),
    };
    drive::wipe_drive(&ctx, params, &Job::detached(), &b)
}

fn erase_calls(m: &MockRunner) -> Vec<Vec<String>> {
    m.calls()
        .into_iter()
        .filter(|c| {
            c.1.first()
                .is_some_and(|a| a == "secureErase" || a == "unmountDisk")
        })
        .map(|c| c.1)
        .collect()
}

#[test]
fn macos_secure_erase_maps_passes_to_diskutil_levels() {
    for (passes, level) in [(1, "1"), (3, "4"), (7, "2"), (35, "3")] {
        let m = MockRunner::new();
        m.on("diskutil", &["info", "/"], CmdOutput::ok(BOOT_INFO));
        m.on("diskutil", &["info", "/dev/disk5"], CmdOutput::ok(EXT_INFO));
        m.on(
            "diskutil",
            &["unmountDisk", "/dev/disk5"],
            CmdOutput::ok("ok"),
        );
        m.on(
            "diskutil",
            &["secureErase", level, "/dev/disk5"],
            CmdOutput::ok("done"),
        );
        mac_wipe(
            &m,
            true,
            json!({"device": "/dev/disk5", "passes": passes, "confirm": "/dev/disk5"}),
        )
        .unwrap_or_else(|e| panic!("{passes}: {e}"));
        let calls = erase_calls(&m);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0], vec!["unmountDisk", "/dev/disk5"]);
        assert_eq!(calls[1], vec!["secureErase", level, "/dev/disk5"]);
    }
}

#[test]
fn macos_refuses_the_boot_disk_and_bad_input_without_erasing() {
    let m = MockRunner::new();
    m.on("diskutil", &["info", "/"], CmdOutput::ok(BOOT_INFO));
    m.on("diskutil", &["info", "/dev/disk5"], CmdOutput::ok(EXT_INFO));
    m.on("diskutil", &["info", "/dev/disk3"], CmdOutput::ok(EXT_INFO));
    m.on("diskutil", &["info", "/dev/disk0"], CmdOutput::ok(EXT_INFO));
    for dev in ["/dev/disk3", "/dev/disk0"] {
        let e = mac_wipe(
            &m,
            true,
            json!({"device": dev, "passes": 1, "confirm": dev}),
        )
        .unwrap_err();
        assert_eq!(e.code, ErrorCode::PermissionDenied, "{dev}");
        assert!(e.message.contains("boot"), "{e}");
    }
    let bad = [
        (
            json!({"device": "/dev/disk5", "passes": 1, "confirm": "disk5"}),
            ErrorCode::InvalidParams,
        ),
        (
            json!({"device": "/dev/disk5s1", "passes": 1, "confirm": "/dev/disk5s1"}),
            ErrorCode::InvalidParams,
        ),
        (
            json!({"device": "/dev/sda", "passes": 1, "confirm": "/dev/sda"}),
            ErrorCode::InvalidParams,
        ),
        (
            json!({"device": "/dev/disk5", "passes": 4, "confirm": "/dev/disk5"}),
            ErrorCode::InvalidParams,
        ),
    ];
    for (p, code) in bad {
        assert_eq!(mac_wipe(&m, true, p.clone()).unwrap_err().code, code, "{p}");
    }
    // not elevated
    let e = mac_wipe(
        &m,
        false,
        json!({"device": "/dev/disk5", "passes": 1, "confirm": "/dev/disk5"}),
    )
    .unwrap_err();
    assert!(e.message.contains("Administrator rights are required"));
    assert!(
        erase_calls(&m).is_empty(),
        "nothing was unmounted or erased"
    );
}

#[test]
fn macos_fails_closed_when_the_boot_disk_is_unknown() {
    let m = MockRunner::new(); // diskutil is not available at all
    let e = mac_wipe(
        &m,
        true,
        json!({"device": "/dev/disk5", "passes": 1, "confirm": "/dev/disk5"}),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::PermissionDenied);
    let m = MockRunner::new();
    m.on("diskutil", &["info", "/"], CmdOutput::ok("nothing useful"));
    let e = mac_wipe(
        &m,
        true,
        json!({"device": "/dev/disk5", "passes": 1, "confirm": "/dev/disk5"}),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::PermissionDenied);
    assert!(erase_calls(&m).is_empty());
}

#[test]
fn macos_stops_when_the_disk_cannot_be_unmounted_or_is_not_whole() {
    let m = MockRunner::new();
    m.on("diskutil", &["info", "/"], CmdOutput::ok(BOOT_INFO));
    m.on("diskutil", &["info", "/dev/disk5"], CmdOutput::ok(EXT_INFO));
    m.on(
        "diskutil",
        &["unmountDisk", "/dev/disk5"],
        CmdOutput::failed(1, "busy"),
    );
    let e = mac_wipe(
        &m,
        true,
        json!({"device": "/dev/disk5", "passes": 1, "confirm": "/dev/disk5"}),
    )
    .unwrap_err();
    assert!(e.message.contains("could not unmount"), "{e}");
    assert!(!erase_calls(&m).iter().any(|c| c[0] == "secureErase"));

    let m = MockRunner::new();
    m.on("diskutil", &["info", "/"], CmdOutput::ok(BOOT_INFO));
    m.on(
        "diskutil",
        &["info", "/dev/disk6"],
        CmdOutput::ok("   Whole:  No\n"),
    );
    let e = mac_wipe(
        &m,
        true,
        json!({"device": "/dev/disk6", "passes": 1, "confirm": "/dev/disk6"}),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidParams);
    assert!(erase_calls(&m).is_empty());
}

// ---------------------------------------------------------------- list_drives

#[test]
fn list_drives_marks_the_volume_holding_the_root_as_system() {
    let (_d, ctx) = bed();
    let v = dispatch(&ctx, "wiper.list_drives", json!({}), &Job::detached()).unwrap();
    let drives = v.as_array().unwrap();
    assert!(!drives.is_empty());
    let mut system = 0;
    for dr in drives {
        assert!(dr["isSystem"].is_boolean());
        assert!(dr["total"].as_u64().unwrap() >= dr["available"].as_u64().unwrap());
        if dr["isSystem"] == true {
            system += 1;
            // the test root lives in a temp dir: its volume must be among the system ones
        }
    }
    let root = &ctx.env.root;
    fs::create_dir_all(root).unwrap();
    let holder = drives
        .iter()
        .filter(|d| is_within(root, Path::new(d["mount"].as_str().unwrap())))
        .max_by_key(|d| d["mount"].as_str().unwrap().len())
        .expect("some volume holds the temp dir");
    assert_eq!(holder["isSystem"], true, "{holder}");
    assert!(system >= 1);
}

#[test]
fn system_mount_rules() {
    let (_d, mut ctx) = bed();
    ctx.env.os = Os::Linux;
    for m in ["/", "/boot", "/boot/efi", "/usr", "/var"] {
        assert!(is_system_mount(&ctx, m, None), "{m}");
    }
    assert!(!is_system_mount(&ctx, "/mnt/usb", None));
    assert!(
        is_system_mount(&ctx, "/mnt/usb", Some("/mnt/usb")),
        "the volume holding the root"
    );
    ctx.env.os = Os::MacOs;
    assert!(is_system_mount(&ctx, "/", None));
    assert!(is_system_mount(&ctx, "/System/Volumes/Data", None));
    assert!(!is_system_mount(&ctx, "/Volumes/USB", None));
    ctx.env.os = Os::Windows;
    ctx.env.root = PathBuf::from("C:\\");
    assert!(is_system_mount(&ctx, "C:\\", None));
    assert!(is_system_mount(&ctx, "c:\\", None));
    assert!(!is_system_mount(&ctx, "D:\\", None));
}
