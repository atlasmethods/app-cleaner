//! A short record of applications that ClearSweep itself uninstalled.
//!
//! `uninstall.remove_leftovers` deletes data folders named after an application, so it is
//! only allowed for applications that were removed through `uninstall.run` (recently, and by
//! this installation of ClearSweep). That keeps a hostile or buggy client from pointing the
//! leftover cleaner at the data of an application that is still in use.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::ctx::Ctx;
use crate::fsutil::{atomic_write, now_unix};

static LOCK: Mutex<()> = Mutex::new(());

const MAX_ENTRIES: usize = 100;
/// Entries older than this are forgotten.
const MAX_AGE_SECS: u64 = 90 * 24 * 3600;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    pub at: u64,
}

fn path(ctx: &Ctx) -> PathBuf {
    ctx.env.data_dir.join("uninstalled-apps.json")
}

fn load(ctx: &Ctx, now: u64) -> Vec<Entry> {
    let Ok(bytes) = fs::read(path(ctx)) else {
        return Vec::new();
    };
    let mut v: Vec<Entry> = serde_json::from_slice(&bytes).unwrap_or_default();
    v.retain(|e| now.saturating_sub(e.at) <= MAX_AGE_SECS);
    v
}

/// Remember that `id` / `name` was just uninstalled.
pub fn record(ctx: &Ctx, id: &str, name: &str, bundle_id: Option<&str>) {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let now = now_unix();
    let mut v = load(ctx, now);
    v.retain(|e| !(e.id == id && e.name == name));
    v.push(Entry {
        id: id.to_string(),
        name: name.to_string(),
        bundle_id: bundle_id.map(str::to_string),
        at: now,
    });
    if v.len() > MAX_ENTRIES {
        let drop = v.len() - MAX_ENTRIES;
        v.drain(..drop);
    }
    // Best effort: losing the record only means leftovers are not offered for removal.
    if let Ok(bytes) = serde_json::to_vec_pretty(&v) {
        let _ = atomic_write(&path(ctx), &bytes);
    }
}

/// The record for exactly this `id` and `name`, if ClearSweep uninstalled it.
pub fn find(ctx: &Ctx, id: &str, name: &str) -> Option<Entry> {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    load(ctx, now_unix())
        .into_iter()
        .rev()
        .find(|e| e.id == id && e.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::MockRunner;

    fn ctx() -> (tempfile::TempDir, Ctx) {
        let d = tempfile::tempdir().unwrap();
        let c = Ctx::test(d.path(), MockRunner::new());
        (d, c)
    }

    #[test]
    fn records_and_finds_exact_matches_only() {
        let (_d, c) = ctx();
        assert!(find(&c, "dpkg:a", "a").is_none());
        record(&c, "dpkg:a", "a", None);
        record(&c, "macapp:/Applications/B.app", "B", Some("org.example.b"));
        assert!(find(&c, "dpkg:a", "a").is_some());
        assert!(find(&c, "dpkg:a", "other").is_none());
        assert!(find(&c, "dpkg:b", "a").is_none());
        assert_eq!(
            find(&c, "macapp:/Applications/B.app", "B")
                .unwrap()
                .bundle_id
                .as_deref(),
            Some("org.example.b")
        );
    }

    #[test]
    fn old_entries_expire_and_the_list_is_capped() {
        let (_d, c) = ctx();
        let old = Entry {
            id: "dpkg:old".into(),
            name: "old".into(),
            bundle_id: None,
            at: now_unix() - MAX_AGE_SECS - 10,
        };
        fs::create_dir_all(&c.env.data_dir).unwrap();
        fs::write(path(&c), serde_json::to_vec(&vec![old]).unwrap()).unwrap();
        assert!(find(&c, "dpkg:old", "old").is_none());
        for i in 0..(MAX_ENTRIES + 5) {
            record(&c, &format!("dpkg:p{i}"), &format!("p{i}"), None);
        }
        let all = load(&c, now_unix());
        assert_eq!(all.len(), MAX_ENTRIES);
        assert!(find(&c, "dpkg:p0", "p0").is_none());
        assert!(find(
            &c,
            &format!("dpkg:p{}", MAX_ENTRIES + 4),
            &format!("p{}", MAX_ENTRIES + 4)
        )
        .is_some());
    }

    #[test]
    fn corrupt_files_are_ignored() {
        let (_d, c) = ctx();
        fs::create_dir_all(&c.env.data_dir).unwrap();
        fs::write(path(&c), "not json").unwrap();
        assert!(find(&c, "x", "y").is_none());
        record(&c, "dpkg:a", "a", None);
        assert!(find(&c, "dpkg:a", "a").is_some());
    }
}
