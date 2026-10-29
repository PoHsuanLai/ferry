//! syncd: ferry's sync daemon, as a library so each part is tested without the daemon.
//!
//! The engine (the dataset seam, the SQLite journal, the scheduler, the driver and the hub of
//! running datasets) is `ferry-engine`'s, and the PIM mirror and the vdir are `ferry-pim-mirror`'s
//! and `ferry-vdir`'s. What is syncd's:
//!
//! - `datasets`: the datasets it carries (contacts, the storage folder, the whole
//!   drives shown in Files, Photos) and the supervisors that turn a grant into a running one.
//! - `keep`: what syncd keeps of the accounts accountd has: the sync switches (each is a grant
//!   accountd asks the person for the first time), "Show in Files" and removal, all through
//!   porter-client. `settings`: the Settings module (`org.quire.SettingsModule1`) over them.
//! - `service`: `org.quire.Sync1` and `org.quire.Storage1`, the bus objects over the hub.
//! - `daemon`: the daemon as one call, [`Config`] (typed; [`Config::from_env`] is the one place the
//!   environment is read), [`Daemon::build`] and [`Daemon::run`]. The binary only parses its
//!   arguments and calls these.
// peephole's caller types (porter-dbus ProcCallers) take the daemon's futures past the default
// depth of 128.
#![recursion_limit = "256"]
pub mod daemon;
pub mod datasets;
pub mod gdrive;
pub mod graph;
pub mod keep;
pub mod paths;
pub mod removal;
pub mod service;
pub mod settings;
pub mod webdav;
pub mod words;

pub use daemon::{Config, Daemon, StartError};

#[cfg(test)]
mod testing;
