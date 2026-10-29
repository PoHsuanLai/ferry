//! The sync engine of design/31 §6, as a library every ferry-style daemon (syncd, callie's
//! calendard) runs its datasets with, so each part is tested without a daemon.
//!
//! - `dataset`: the local side of a sync (`Dataset`), and a dataset's name.
//! - `journal`: the SQLite journal of one dataset of one account (items, anchor, tombstones,
//!   conflicts), every change one transaction.
//! - `engine`: one dataset against one `ferry_sync::Replica`, a cycle at a time.
//! - `scheduler`: when to look next; pure.
//! - `driver`: one engine run on the scheduler's wake-ups.
//! - `hub`, `resolve`: the registry of running datasets, who may see each, its latest status and
//!   its pause switch; the requests a bus object sends a driver. The bus object itself is the
//!   daemon's.
//! - `clock`, `account`, `rescan`: the clock the engine dates by, an account as a directory
//!   name, and the test-build knob for the supervisors' rescan.
//!
//! No bus, no account stack: the crate reaches the file system (the journal), tokio and the
//! caller type porter-callers defines.

pub mod account;
pub mod clock;
pub mod dataset;
pub mod driver;
pub mod engine;
pub mod hub;
pub mod journal;
pub mod rescan;
pub mod resolve;
pub mod scheduler;

#[cfg(test)]
mod testing;
