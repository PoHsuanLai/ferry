//! Settling a stored conflict: the request that goes from the hub to the dataset's driver (which
//! owns the engine and settles it between cycles, so nothing races a cycle writing the journal).
//! The words a bus caller says it with (`keep_local`, `keep_remote`) are the bus object's.

use ferry_sync::Resolution;
use tokio::sync::oneshot;

/// A stored conflict's number: the one a `Conflict` signal carries under `number`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConflictNumber(pub i64);

/// The side that wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct How(pub Resolution);

/// Why a conflict was not settled.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SettleError {
    /// There is no such dataset for the caller (never registered, gone, or not the caller's to
    /// see): one answer for all, so a name is never confirmed to a caller who may not have it.
    NoSuchDataset,
    /// The caller sees the dataset but is not its owning app.
    NotOwner,
    /// The conflict is unknown or already settled.
    NoSuchConflict,
    /// The journal or the local side failed.
    Failed(String),
}

/// One request to a dataset's driver.
#[derive(Debug)]
pub struct Settle {
    /// The conflict.
    pub number: ConflictNumber,
    /// The side that wins.
    pub how: How,
    /// Where the driver answers.
    pub reply: oneshot::Sender<Result<(), SettleError>>,
}

/// Why a discard was not let through.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfirmError {
    /// There is no such dataset for the caller (as [`SettleError::NoSuchDataset`]).
    NoSuchDataset,
    /// Nothing is held for confirmation: it never was, or the discard was already confirmed.
    NothingHeld,
}

/// One request to a dataset's driver: let the held discard through.
#[derive(Debug)]
pub struct Confirm {
    /// Where the driver answers.
    pub reply: oneshot::Sender<Result<(), ConfirmError>>,
}

/// Why "Sync now" did not start a cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SyncNowError {
    /// There is no such dataset (never registered, or gone).
    NoSuchDataset,
    /// The person paused the dataset: it does not cycle until it is resumed.
    Paused,
}
