//! Method registry and dispatcher shared by every transport.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::ctx::Ctx;
use crate::error::{ApiError, ErrorCode, Result};
use crate::job::{Job, ProgressEvent};

/// An API method: JSON params in, JSON result out.
pub type Handler = fn(&Ctx, Value, &Job) -> Result<Value>;

#[derive(Default)]
pub struct Registry {
    handlers: HashMap<&'static str, Handler>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `name`. Panics on duplicates (programmer error caught by tests).
    pub fn add(&mut self, name: &'static str, handler: Handler) {
        let prev = self.handlers.insert(name, handler);
        assert!(prev.is_none(), "duplicate API method registered: {name}");
    }

    /// Register every name in `names` as a not-yet-implemented stub.
    pub fn stubs(&mut self, names: &[&'static str]) {
        for n in names {
            self.add(n, not_implemented);
        }
    }

    pub fn get(&self, name: &str) -> Option<Handler> {
        self.handlers.get(name).copied()
    }

    /// Sorted method names.
    pub fn names(&self) -> Vec<&'static str> {
        let mut v: Vec<_> = self.handlers.keys().copied().collect();
        v.sort_unstable();
        v
    }
}

/// Handler used by planned-but-unbuilt methods. [`dispatch`] fills in the method name.
pub fn not_implemented(_: &Ctx, _: Value, _: &Job) -> Result<Value> {
    Err(ApiError::not_implemented(""))
}

fn methods_handler(_: &Ctx, _: Value, _: &Job) -> Result<Value> {
    Ok(json!(registry().names()))
}

/// Built-in diagnostic method: sleeps `ms` milliseconds (default 100) in 10ms steps,
/// emitting progress and honoring cancellation. Used by transport tests.
fn sleep_handler(_: &Ctx, params: Value, job: &Job) -> Result<Value> {
    let ms = params
        .get("ms")
        .and_then(Value::as_u64)
        .unwrap_or(100)
        .min(60_000);
    let steps = (ms / 10).max(1);
    for i in 0..steps {
        job.check_cancelled()?;
        job.progress(
            ProgressEvent::new("sleep")
                .fraction(i as f64 / steps as f64)
                .counts(i, steps),
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Ok(json!({ "slept": ms }))
}

/// Global registry, built once from every feature's `register`.
pub fn registry() -> &'static Registry {
    static REG: OnceLock<Registry> = OnceLock::new();
    REG.get_or_init(|| {
        let mut r = Registry::new();
        r.add("api.methods", methods_handler);
        r.add("api.sleep", sleep_handler);
        crate::features::register_all(&mut r);
        r
    })
}

/// Look up `method` and run it.
pub fn dispatch(ctx: &Ctx, method: &str, params: Value, job: &Job) -> Result<Value> {
    let handler = registry()
        .get(method)
        .ok_or_else(|| ApiError::not_found(format!("unknown method `{method}`")))?;
    job.check_cancelled()?;
    handler(ctx, params, job).map_err(|mut e| {
        if e.code == ErrorCode::NotImplemented && e.message.is_empty() {
            e.message = format!("`{method}` is not implemented yet");
        }
        e
    })
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
    fn unknown_method_is_not_found() {
        let (_d, c) = ctx();
        let e = dispatch(&c, "nope.nothing", Value::Null, &Job::detached()).unwrap_err();
        assert_eq!(e.code, ErrorCode::NotFound);
    }

    #[test]
    fn api_methods_lists_everything_sorted() {
        let (_d, c) = ctx();
        let v = dispatch(&c, "api.methods", Value::Null, &Job::detached()).unwrap();
        let names: Vec<String> = serde_json::from_value(v).unwrap();
        assert_eq!(names.len(), registry().names().len());
        assert!(names.windows(2).all(|w| w[0] < w[1]));
        assert!(names.contains(&"api.methods".to_string()));
    }

    #[test]
    fn every_planned_method_is_registered() {
        let planned = crate::features::planned_methods();
        assert!(planned.len() > 40);
        for m in planned {
            assert!(registry().get(m).is_some(), "missing method {m}");
        }
        // Every registered method belongs to a known namespace.
        let prefixes: Vec<&str> = crate::features::FEATURES.iter().map(|f| f.0).collect();
        for n in registry().names() {
            let ns = n.split('.').next().unwrap();
            assert!(
                ns == "api" || prefixes.contains(&ns),
                "unexpected namespace in {n}"
            );
        }
    }

    #[test]
    fn stubs_return_not_implemented_with_method_name() {
        let (_d, c) = ctx();
        let e = dispatch(&c, "scheduler.list", Value::Null, &Job::detached()).unwrap_err();
        assert_eq!(e.code, ErrorCode::NotImplemented);
        assert!(e.message.contains("scheduler.list"), "{}", e.message);
    }

    #[test]
    fn cancelled_job_short_circuits() {
        let (_d, c) = ctx();
        let job = Job::detached();
        job.token().cancel();
        let e = dispatch(&c, "api.sleep", json!({"ms": 1000}), &job).unwrap_err();
        assert_eq!(e.code, ErrorCode::Cancelled);
    }

    #[test]
    #[should_panic(expected = "duplicate")]
    fn duplicate_registration_panics() {
        let mut r = Registry::new();
        r.add("a.b", not_implemented);
        r.add("a.b", not_implemented);
    }
}
