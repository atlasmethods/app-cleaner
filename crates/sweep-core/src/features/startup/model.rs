//! Types shared by the startup manager's platform modules.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// XDG autostart entry, `Run` registry value, startup-folder shortcut.
    Autostart,
    /// systemd unit or Windows service.
    Service,
    ScheduledTask,
    ContextMenu,
    /// `@reboot` crontab line.
    Cron,
    LaunchAgent,
    LaunchDaemon,
    LoginItem,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    User,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Impact {
    High,
    Medium,
    Low,
    Unknown,
}

/// One row of `startup.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupItem {
    /// Stable across enable/disable; the only thing a client sends back.
    pub id: String,
    pub name: String,
    pub command: String,
    /// File path or registry key the item lives in.
    pub location: String,
    pub kind: Kind,
    pub scope: Scope,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    pub impact: Impact,
    pub can_disable: bool,
    pub can_delete: bool,
    /// Never touched by ClearSweep: needed for the system to work.
    pub critical: bool,
    /// Shown next to the toggle (for example "disabling SSH stops remote logins").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

impl StartupItem {
    pub fn new(id: impl Into<String>, name: impl Into<String>, kind: Kind, scope: Scope) -> Self {
        StartupItem {
            id: id.into(),
            name: name.into(),
            command: String::new(),
            location: String::new(),
            kind,
            scope,
            enabled: true,
            publisher: None,
            impact: Impact::Unknown,
            can_disable: true,
            can_delete: false,
            critical: false,
            warning: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinHive {
    Hkcu,
    Hklm,
}

impl WinHive {
    pub fn name(self) -> &'static str {
        match self {
            WinHive::Hkcu => "HKCU",
            WinHive::Hklm => "HKLM",
        }
    }
    pub fn needs_admin(self) -> bool {
        self == WinHive::Hklm
    }
}

/// Which Windows startup registry key a value lives in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunLoc {
    pub hive: WinHive,
    /// Key path below the hive, without leading backslash.
    pub key: String,
    /// `StartupApproved` subkey name (`Run`, `Run32`, `StartupFolder`); `None` for `RunOnce`.
    pub approved: Option<&'static str>,
}

/// What an item is, on the server side: everything needed to act on it. Built by `collect`
/// from the live system, never from client input.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Xdg {
        /// File name, e.g. `slack.desktop`.
        file: String,
        user_path: PathBuf,
        system_path: Option<PathBuf>,
    },
    Systemd {
        unit: String,
        user: bool,
    },
    Cron {
        /// The underlying `@reboot ...` line without any disabled-prefix.
        line: String,
        /// Which of several identical lines.
        nth: usize,
    },
    WinRun {
        loc: RunLoc,
        name: String,
    },
    WinFolder {
        path: PathBuf,
        file: String,
        all_users: bool,
    },
    WinTask {
        name: String,
    },
    WinService {
        name: String,
    },
    WinContext {
        hive: WinHive,
        /// `SOFTWARE\Classes\<class>\shellex\ContextMenuHandlers`
        key: String,
        /// Sub-key name as it is in the registry now (with a leading `-` when disabled).
        handler: String,
    },
    MacPlist {
        path: PathBuf,
        label: String,
        /// `gui/<uid>` for agents, `system` for daemons.
        domain: String,
        daemon: bool,
        /// Lives in the user's own `~/Library/LaunchAgents`.
        in_home: bool,
    },
    MacLogin {
        name: String,
        path: String,
    },
}

/// A startup item together with what is needed to act on it.
#[derive(Debug, Clone)]
pub struct Entry {
    pub item: StartupItem,
    pub target: Target,
    /// Executable (path or bare name) used to find the item's processes and to group
    /// items into apps.
    pub exe: Option<String>,
    /// Icon name from a desktop entry, when there is one.
    pub icon: Option<String>,
}

impl Entry {
    pub fn new(item: StartupItem, target: Target) -> Self {
        Entry {
            item,
            target,
            exe: None,
            icon: None,
        }
    }
}

/// How `set_enabled` should treat things that are running right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ToggleOpts {
    /// Also stop the running item (`launchctl bootout` on macOS). Off for the optimizer,
    /// which stops processes itself and must never stop anything on `enforce`.
    pub stop_now: bool,
}
