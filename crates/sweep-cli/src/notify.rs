//! Desktop notifications for the background agent (Linux D-Bus, macOS Notification Center,
//! Windows toasts) through `notify-rust`.
//!
//! A notification that cannot be shown (no notification daemon, no session bus, a headless
//! server) is logged to stderr and otherwise ignored: it must never stop the agent.
//!
//! Testing hook: when `CLEARSWEEP_NOTIFY_FILE` is set, notifications are appended to that
//! file (`title<TAB>body` per line) instead of being sent to the desktop.

use std::io::Write;
use sweep_core::agent::Notifier;

#[derive(Debug, Default, Clone, Copy)]
pub struct DesktopNotifier;

impl Notifier for DesktopNotifier {
    fn notify(&self, title: &str, body: &str) {
        if let Some(path) = std::env::var_os("CLEARSWEEP_NOTIFY_FILE").filter(|p| !p.is_empty()) {
            let line = format!("{}\t{}\n", one_line(title), one_line(body));
            if let Err(e) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .and_then(|mut f| f.write_all(line.as_bytes()))
            {
                eprintln!("clearsweep: could not record the notification: {e}");
            }
            return;
        }
        let (title, body) = (title.to_string(), body.to_string());
        // A panic inside the notification stack must not take the agent down either.
        let shown = std::panic::catch_unwind(move || {
            notify_rust::Notification::new()
                .appname("ClearSweep")
                .summary(&title)
                .body(&body)
                .show()
                .map(|_| ())
        });
        match shown {
            Ok(Ok(())) => {}
            Ok(Err(e)) => eprintln!("clearsweep: notification not shown: {e}"),
            Err(_) => {
                eprintln!("clearsweep: notification not shown: the notification library panicked")
            }
        }
    }
}

fn one_line(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_line_has_no_separators() {
        assert_eq!(one_line("a\tb\nc"), "a b c");
    }
}
