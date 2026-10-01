//! Backups taken before anything is removed: `<data>/backups/<kind>-<timestamp>/` with a
//! `manifest.json` (`{kind, createdAt, description, items: [...]}`) and the saved files under
//! `files/`. Shared with the browser plugins feature.
//!
//! A backup is only useful if it exists before the change, so the pattern is always
//! `create` -> stage files / exports -> `commit` (writes the manifest) -> change something;
//! any failure before `commit` aborts the whole operation and removes the half-made folder.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

use crate::ctx::Ctx;
use crate::error::{ApiError, Result};
use crate::fsutil::{atomic_write, now_rfc3339, now_unix};

pub struct Backup {
    pub id: String,
    pub dir: PathBuf,
    kind: String,
    description: String,
    items: Vec<Value>,
    /// Extra top-level manifest fields (`startup`, `plugin`): what was removed and where from.
    fields: Vec<(String, Value)>,
    staged: usize,
}

/// Does `id` look like a backup folder name we created (`startup-1700000000`,
/// `plugins-1700000000-2`)? Guards against path tricks in ids that come from a client.
pub fn valid_backup_id(kind: &str, id: &str) -> bool {
    let Some(rest) = id.strip_prefix(kind).and_then(|r| r.strip_prefix('-')) else {
        return false;
    };
    let mut parts = rest.split('-');
    let first_ok = parts
        .next()
        .is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    let second_ok = parts
        .next()
        .is_none_or(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    first_ok && second_ok && parts.next().is_none()
}

pub fn backups_root(ctx: &Ctx) -> PathBuf {
    ctx.env.data_dir.join("backups")
}

impl Backup {
    /// Make a fresh backup folder. Fails (and leaves nothing behind) when the backup area
    /// cannot be written.
    pub fn create(ctx: &Ctx, kind: &str, description: &str) -> Result<Backup> {
        let root = backups_root(ctx);
        fs::create_dir_all(&root)
            .map_err(|e| ApiError::io(format!("could not create the backup folder: {e}")))?;
        let ts = now_unix();
        for n in 0..1000u32 {
            let id = if n == 0 {
                format!("{kind}-{ts}")
            } else {
                format!("{kind}-{ts}-{n}")
            };
            let dir = root.join(&id);
            match fs::create_dir(&dir) {
                Ok(()) => {
                    if let Err(e) = fs::create_dir(dir.join("files")) {
                        let _ = fs::remove_dir(&dir);
                        return Err(ApiError::io(format!("could not create the backup: {e}")));
                    }
                    return Ok(Backup {
                        id,
                        dir,
                        kind: kind.to_string(),
                        description: description.to_string(),
                        items: Vec::new(),
                        fields: Vec::new(),
                        staged: 0,
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    return Err(ApiError::io(format!("could not create the backup: {e}")));
                }
            }
        }
        Err(ApiError::io("could not choose a backup folder name"))
    }

    /// Path inside the backup for a file some other tool (`reg export`) will write.
    pub fn stage_path(&mut self, name: &str) -> PathBuf {
        self.staged += 1;
        let safe: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.dir
            .join("files")
            .join(format!("{}-{safe}", self.staged))
    }

    /// Copy one file into the backup; returns the path of the copy.
    pub fn copy_file(&mut self, src: &Path) -> Result<PathBuf> {
        let name = src
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into());
        let dst = self.stage_path(&name);
        fs::copy(src, &dst)
            .map_err(|e| ApiError::io(format!("could not back up {} first: {e}", src.display())))?;
        Ok(dst)
    }

    /// Copy a directory tree into the backup (symlinks are skipped, never followed).
    pub fn copy_tree(&mut self, src: &Path) -> Result<PathBuf> {
        let name = src
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "dir".into());
        let dst = self.stage_path(&name);
        copy_dir(src, &dst)
            .map_err(|e| ApiError::io(format!("could not back up {} first: {e}", src.display())))?;
        Ok(dst)
    }

    pub fn write_text(&mut self, name: &str, text: &str) -> Result<PathBuf> {
        let dst = self.stage_path(name);
        fs::write(&dst, text)
            .map_err(|e| ApiError::io(format!("could not write the backup: {e}")))?;
        Ok(dst)
    }

    /// Path of a staged file relative to the backup folder (as stored in the manifest).
    pub fn rel(&self, p: &Path) -> String {
        p.strip_prefix(&self.dir)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/")
    }

    pub fn add_item(&mut self, item: Value) {
        self.items.push(item);
    }

    /// Record an extra top-level manifest field (describes what the backup is of).
    pub fn set_field(&mut self, key: &str, value: Value) {
        self.fields.push((key.to_string(), value));
    }

    /// Write `manifest.json`. After this the backup is complete and the caller may change
    /// the system.
    pub fn commit(self) -> Result<Committed> {
        let mut manifest = json!({
            "kind": self.kind,
            "createdAt": now_rfc3339(),
            "createdAtUnix": now_unix(),
            "description": self.description,
            "items": self.items,
        });
        for (k, v) in &self.fields {
            // The fixed keys above always win.
            if manifest.get(k).is_none() {
                manifest[k.as_str()] = v.clone();
            }
        }
        let text = serde_json::to_vec_pretty(&manifest)?;
        if let Err(e) = atomic_write(&self.dir.join("manifest.json"), &text) {
            let _ = fs::remove_dir_all(&self.dir);
            return Err(ApiError::io(format!("could not write the backup: {e}")));
        }
        Ok(Committed {
            id: self.id,
            dir: self.dir,
        })
    }

    /// Give up: remove the folder made by `create` (nothing else is touched).
    pub fn abort(self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[derive(Debug, Clone)]
pub struct Committed {
    pub id: String,
    pub dir: PathBuf,
}

pub(crate) fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for e in fs::read_dir(src)? {
        let e = e?;
        let ft = e.file_type()?;
        let to = dst.join(e.file_name());
        if ft.is_dir() {
            copy_dir(&e.path(), &to)?;
        } else if ft.is_file() {
            fs::copy(e.path(), &to)?;
        }
    }
    Ok(())
}

/// Load and check a manifest of the given `kind`.
pub fn read_manifest(ctx: &Ctx, kind: &str, id: &str) -> Result<(PathBuf, Value)> {
    if !valid_backup_id(kind, id) {
        return Err(ApiError::invalid_params("not a valid backup id"));
    }
    let dir = backups_root(ctx).join(id);
    let text = fs::read_to_string(dir.join("manifest.json"))
        .map_err(|_| ApiError::not_found(format!("backup `{id}` not found")))?;
    let v: Value = serde_json::from_str(&text)
        .map_err(|e| ApiError::io(format!("backup `{id}` is damaged: {e}")))?;
    if v.get("kind").and_then(Value::as_str) != Some(kind) {
        return Err(ApiError::invalid_params(format!(
            "`{id}` is not a {kind} backup"
        )));
    }
    Ok((dir, v))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::MockRunner;

    #[test]
    fn ids_are_validated() {
        assert!(valid_backup_id("startup", "startup-1700000000"));
        assert!(valid_backup_id("startup", "startup-1700000000-3"));
        assert!(!valid_backup_id("startup", "startup-1700000000-3-4"));
        assert!(!valid_backup_id("startup", "startup-../x"));
        assert!(!valid_backup_id("startup", "plugins-1"));
        assert!(!valid_backup_id("startup", "startup-"));
        assert!(!valid_backup_id("startup", "startup-1/2"));
    }

    #[test]
    fn create_stage_commit_writes_manifest_and_unique_dirs() {
        let d = tempfile::tempdir().unwrap();
        let ctx = Ctx::test(d.path(), MockRunner::new());
        let src = d.path().join("a.desktop");
        fs::write(&src, "x").unwrap();
        let mut b1 = Backup::create(&ctx, "startup", "test").unwrap();
        let c = b1.copy_file(&src).unwrap();
        assert_eq!(fs::read(&c).unwrap(), b"x");
        b1.add_item(json!({"type": "file", "backup": b1.rel(&c)}));
        let done = b1.commit().unwrap();
        let b2 = Backup::create(&ctx, "startup", "second").unwrap();
        assert_ne!(done.id, b2.id);
        let (_, m) = read_manifest(&ctx, "startup", &done.id).unwrap();
        assert_eq!(m["kind"], "startup");
        assert_eq!(m["items"][0]["backup"], "files/1-a.desktop");
        assert!(m["createdAt"].as_str().unwrap().ends_with('Z'));
        b2.abort();
    }

    #[test]
    fn create_fails_when_backup_area_is_unwritable() {
        let d = tempfile::tempdir().unwrap();
        let ctx = Ctx::test(d.path(), MockRunner::new());
        // `data/backups` exists as a regular file.
        fs::create_dir_all(&ctx.env.data_dir).unwrap();
        fs::write(ctx.env.data_dir.join("backups"), "not a dir").unwrap();
        assert!(Backup::create(&ctx, "startup", "x").is_err());
    }
}
