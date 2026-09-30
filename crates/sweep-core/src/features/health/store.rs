//! The last result: `<data_dir>/health-last.json`, so the Home tab can show the previous
//! score at once. A damaged or unreadable file simply reads as "never scanned".

use std::sync::Mutex;

use crate::ctx::Ctx;
use crate::error::Result;
use crate::fsutil::atomic_write;

use super::model::HealthReport;

static LOCK: Mutex<()> = Mutex::new(());

fn path(ctx: &Ctx) -> std::path::PathBuf {
    ctx.env.data_dir.join("health-last.json")
}

pub fn load(ctx: &Ctx) -> Option<HealthReport> {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let bytes = std::fs::read(path(ctx)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn save(ctx: &Ctx, report: &HealthReport) -> Result<()> {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    atomic_write(&path(ctx), &serde_json::to_vec_pretty(report)?)?;
    Ok(())
}

/// Forget the stored result (it no longer describes the machine).
pub fn clear(ctx: &Ctx) {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _ = std::fs::remove_file(path(ctx));
}
