//! Cleaning history: `<data_dir>/history.json`, newest last on disk, capped at 100.

use std::fs;
use std::sync::Mutex;

use crate::ctx::Ctx;
use crate::error::Result;
use crate::features::cleaner::model::HistoryEntry;
use crate::fsutil::atomic_write;

pub const MAX_ENTRIES: usize = 100;

static LOCK: Mutex<()> = Mutex::new(());

fn path(ctx: &Ctx) -> std::path::PathBuf {
    ctx.env.data_dir.join("history.json")
}

fn read(ctx: &Ctx) -> Vec<HistoryEntry> {
    let Ok(bytes) = fs::read(path(ctx)) else {
        return Vec::new();
    };
    // Tolerate a damaged file: drop it rather than blocking cleaning.
    serde_json::from_slice(&bytes).unwrap_or_default()
}

/// Append an entry, keeping only the newest [`MAX_ENTRIES`].
pub fn append(ctx: &Ctx, entry: HistoryEntry) -> Result<()> {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = read(ctx);
    all.push(entry);
    if all.len() > MAX_ENTRIES {
        let extra = all.len() - MAX_ENTRIES;
        all.drain(..extra);
    }
    atomic_write(&path(ctx), &serde_json::to_vec_pretty(&all)?)?;
    Ok(())
}

/// Newest first.
pub fn list(ctx: &Ctx, limit: Option<usize>) -> Vec<HistoryEntry> {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = read(ctx);
    all.reverse();
    all.truncate(limit.unwrap_or(MAX_ENTRIES).min(MAX_ENTRIES));
    all
}
