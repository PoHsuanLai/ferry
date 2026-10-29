//! The collections running now, for whoever serves them: which calendars (address books, task
//! lists) a daemon mirrors, under what names, and where their files are. The supervisor fills it
//! at the end of every look; a reader takes a copy.

use ferry_engine::account::AccountDir;
use ferry_engine::dataset::DatasetId;
use ferry_vdir::PimKind;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

/// One collection a mirror keeps running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collection {
    /// The account it belongs to.
    pub account: AccountDir,
    /// What it holds.
    pub kind: PimKind,
    /// Its dataset (`<account>/<dataset>` is its name in the hub).
    pub dataset: DatasetId,
    /// Its directory under the account's vdir.
    pub dir: String,
    /// The name the person gave it, as the source last said.
    pub displayname: Option<String>,
    /// Its colour as the source last said.
    pub color: Option<String>,
    /// Its directory on disk.
    pub root: PathBuf,
}

/// The collections running now. Cheap to clone: the clones are one catalog.
#[derive(Debug, Clone, Default)]
pub struct Catalog(Arc<Mutex<Vec<Collection>>>);

impl Catalog {
    /// A copy of the collections running now.
    pub fn collections(&self) -> Vec<Collection> {
        self.held().clone()
    }

    pub(crate) fn replace(&self, all: Vec<Collection>) {
        *self.held() = all;
    }

    fn held(&self) -> std::sync::MutexGuard<'_, Vec<Collection>> {
        // Every critical section is a copy or a swap.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
