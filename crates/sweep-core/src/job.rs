//! Job handle passed to every API handler: progress reporting and cooperative cancellation.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::error::{ApiError, Result};

/// Cooperative cancellation flag, cheap to clone.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// A progress update emitted while a job runs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressEvent {
    /// Short machine-friendly phase name, e.g. "scan".
    pub stage: String,
    /// 0.0..=1.0 when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fraction: Option<f64>,
    /// Human readable status line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

impl ProgressEvent {
    pub fn new(stage: impl Into<String>) -> Self {
        Self {
            stage: stage.into(),
            ..Self::default()
        }
    }
    pub fn fraction(mut self, f: f64) -> Self {
        self.fraction = Some(f.clamp(0.0, 1.0));
        self
    }
    pub fn message(mut self, m: impl Into<String>) -> Self {
        self.message = Some(m.into());
        self
    }
    pub fn counts(mut self, current: u64, total: u64) -> Self {
        self.current = Some(current);
        self.total = Some(total);
        self
    }
}

type Sink = Box<dyn Fn(ProgressEvent) + Send + Sync>;

/// Handle for one running API call.
pub struct Job {
    token: CancelToken,
    sink: Option<Sink>,
}

impl Job {
    /// A job that discards progress and is never cancelled unless `token` is.
    pub fn detached() -> Self {
        Self {
            token: CancelToken::new(),
            sink: None,
        }
    }
    pub fn new(token: CancelToken, sink: impl Fn(ProgressEvent) + Send + Sync + 'static) -> Self {
        Self {
            token,
            sink: Some(Box::new(sink)),
        }
    }
    pub fn with_token(token: CancelToken) -> Self {
        Self { token, sink: None }
    }
    pub fn progress(&self, ev: ProgressEvent) {
        if let Some(s) = &self.sink {
            s(ev);
        }
    }
    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }
    /// `Err(Cancelled)` when cancellation was requested; use with `?` in loops.
    pub fn check_cancelled(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(ApiError::cancelled())
        } else {
            Ok(())
        }
    }
    pub fn token(&self) -> &CancelToken {
        &self.token
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn cancel_token_is_shared_between_clones() {
        let t = CancelToken::new();
        let t2 = t.clone();
        assert!(!t.is_cancelled());
        t2.cancel();
        assert!(t.is_cancelled());
    }

    #[test]
    fn progress_reaches_sink_and_check_cancelled_works() {
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = got.clone();
        let token = CancelToken::new();
        let job = Job::new(token.clone(), move |e| g.lock().unwrap().push(e));
        job.progress(ProgressEvent::new("scan").fraction(2.0));
        assert_eq!(got.lock().unwrap()[0].fraction, Some(1.0));
        assert!(job.check_cancelled().is_ok());
        token.cancel();
        assert!(job.check_cancelled().is_err());
    }

    #[test]
    fn progress_event_camel_case() {
        let v = serde_json::to_value(ProgressEvent::new("x").counts(1, 2)).unwrap();
        assert_eq!(v, serde_json::json!({"stage":"x","current":1,"total":2}));
    }
}
