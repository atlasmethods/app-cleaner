//! ClearSweep core: all cleaning logic lives here, free of UI/transport dependencies.
//!
//! Every feature exposes JSON-in / JSON-out methods through the [`api`] registry, which
//! is shared by the desktop app (Tauri IPC), the browser server (HTTP) and the CLI.

pub mod api;
pub mod ctx;
pub mod elevate;
pub mod error;
pub mod features;
pub mod fsutil;
pub mod job;
pub mod pkgutil;
pub mod procs;
pub mod runner;
pub mod safety;
#[cfg(any(test, feature = "testutil"))]
pub mod testutil;

pub use api::{dispatch, registry, Handler, Registry};
pub use ctx::{Ctx, Env, Os};
pub use error::{ApiError, ErrorCode, Result};
pub use job::{CancelToken, Job, ProgressEvent};
pub use procs::{ProcInfo, ProcessSource, SystemProcesses};
pub use runner::{CmdOutput, CommandRunner, MockRunner, SystemRunner};
