//! One module per feature; each exposes `register(&mut Registry)`.

use crate::api::Registry;

pub mod browser_plugins;
pub mod cleaner;
pub mod cookies;
pub mod disk_analyzer;
pub mod driver_updater;
pub mod duplicates;
pub mod health;
pub mod optimizer;
pub mod registry_cleaner;
pub mod restore;
pub mod scheduler;
pub mod secure_delete;
pub mod settings;
pub mod smart_cleaning;
pub mod software_updater;
pub mod startup;
pub mod sysinfo;
pub mod system;
pub mod uninstall;
pub mod wiper;

/// Namespace prefix and stub method list for every feature (`sysinfo` is fully implemented).
pub const FEATURES: &[(&str, &[&str])] = &[
    ("health", health::METHODS),
    ("cleaner", cleaner::METHODS),
    ("cookies", cookies::METHODS),
    ("registry_cleaner", registry_cleaner::METHODS),
    ("uninstall", uninstall::METHODS),
    ("software_updater", software_updater::METHODS),
    ("driver_updater", driver_updater::METHODS),
    ("optimizer", optimizer::METHODS),
    ("startup", startup::METHODS),
    ("browser_plugins", browser_plugins::METHODS),
    ("disk_analyzer", disk_analyzer::METHODS),
    ("duplicates", duplicates::METHODS),
    ("restore", restore::METHODS),
    ("wiper", wiper::METHODS),
    ("secure_delete", secure_delete::METHODS),
    ("smart_cleaning", smart_cleaning::METHODS),
    ("scheduler", scheduler::METHODS),
    ("settings", settings::METHODS),
    ("sysinfo", sysinfo::METHODS),
    ("system", system::METHODS),
];

pub fn register_all(r: &mut Registry) {
    health::register(r);
    cleaner::register(r);
    cookies::register(r);
    registry_cleaner::register(r);
    uninstall::register(r);
    software_updater::register(r);
    driver_updater::register(r);
    optimizer::register(r);
    startup::register(r);
    browser_plugins::register(r);
    disk_analyzer::register(r);
    duplicates::register(r);
    restore::register(r);
    wiper::register(r);
    secure_delete::register(r);
    smart_cleaning::register(r);
    scheduler::register(r);
    settings::register(r);
    sysinfo::register(r);
    system::register(r);
}

/// Every planned method name across all features.
pub fn planned_methods() -> Vec<&'static str> {
    FEATURES
        .iter()
        .flat_map(|(_, m)| m.iter().copied())
        .collect()
}
