//! What syncd keeps of the accounts the person has. The accounts are accountd's: syncd lists
//! them, asks accountd for the grants it needs and reads its own, all through porter-client
//! (`Grants`, `Accounts`), over the bus in the daemon and in process in tests. syncd holds no
//! account, no token and no sign-in of its own; what it keeps is a switch per account and class
//! in the grants accountd holds for it.
//!
//! - [`Keep::set_sync`]: the sync switches. On asks accountd for the grant of the class
//!   (`Grants::request_grant`): accountd's consent sheet shows the first time and its answer is
//!   kept, so a later turning on finds the grant and asks nothing. Off takes the grant back, and
//!   the mirror stops at the supervisor's next look; the files stay.
//! - [`Keep::show_in_files`]: "Show in Files". On asks the person for the whole drive in a
//!   sign-in of its own when the account was not given it yet (`Grants::extend_access`), then
//!   asks for the grant of it, and the supervisor lists the drive at its next look; off takes the
//!   grant back and keeps the listing, and the access stays given, so on again asks nothing.
//! - [`Keep::keep_everything`]: "Keep Everything Downloaded" for a drive shown in Files, kept in
//!   syncd's settings so it holds across restarts (the drive does the keeping:
//!   `ferry_drive::Drive::set_keep_all`).
//! - [`Keep::add_and_keep`]: "Add Account": accountd's add-account sheet, then the first thing
//!   the account offers for syncd to keep switched on.
//! - [`Keep::remove`]: forgets an account here: stops its datasets, deletes its journals and
//!   mirrors ([`crate::removal::wipe`]) and takes back syncd's grants of it. The account stays in
//!   accountd, which the person removes it from.

mod classes;

pub use classes::{
    SyncClass, claims_whole_drive, drive_allowed, drive_key, holds, keys_of, lists_whole_drive,
    need_for, photos_key, sync_allowed, sync_app, sync_key, sync_offers,
};

use crate::datasets::drive::{DriveSet, Kept};
use crate::paths::{AccountDir, Paths};
use crate::removal;
use crate::service::Hub;
use porter_capability::{CapabilityKind, Toggle};
use porter_client::{
    Accounts, AccountsError, AccountsLink, ConsentOffer, Grants, GrantsError, GrantsLink,
    ListedAccount, Porter,
};
use porter_core::consent::{Grant, Usage};
use porter_core::{Account, AccountId, GrantKey, ProviderHint};
use porter_wire::{ParentWindow, Refusal};
use std::sync::Arc;

/// Why "Add Account" did not end in an account.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ContinueError {
    /// The person closed the sheet without finishing the sign-in.
    #[error("the sign-in was closed before it finished")]
    Cancelled,
    /// An add-account sheet of syncd's is open already.
    #[error("another sign-in is open")]
    Busy,
    /// The sign-in could not be finished.
    #[error("the account could not be signed in")]
    Failed,
    /// accountd could not be reached.
    #[error("the account service could not be reached")]
    Unreachable,
}

/// Why a sync switch did not change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SwitchError {
    /// accountd has no such account.
    #[error("that account is not one of the accounts")]
    UnknownAccount,
    /// The account offers nothing of this kind for syncd to keep.
    #[error("this account has nothing of that kind to keep")]
    NothingToKeep,
    /// The grant could not be recorded or taken back.
    #[error("that could not be saved")]
    Unsaved,
    /// The person did not give the access the switch needs (they closed the sheet or the page,
    /// refused it, or it could not be asked).
    #[error("the access was not given")]
    NotGiven,
    /// accountd could not be reached.
    #[error("the account service could not be reached")]
    Unreachable,
}

/// Why an account was not removed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RemoveError {
    /// accountd has no such account.
    #[error("that account is not one of the accounts")]
    Unknown,
    /// Its journals or mirrors could not be deleted; nothing else was removed.
    #[error("its files on this computer could not be deleted: {0}")]
    Files(String),
    /// Its grants could not be taken back.
    #[error("its permissions could not be taken back")]
    Grants,
    /// accountd could not be reached.
    #[error("the account service could not be reached")]
    Unreachable,
}

/// How "Add Account" ended well.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Continued {
    /// accountd added the account.
    SignedIn(AccountId),
    /// accountd already had this account; nothing was added.
    AlreadyHere(AccountId),
}

impl Continued {
    /// The account either way.
    pub fn account(&self) -> &AccountId {
        match self {
            Continued::SignedIn(id) | Continued::AlreadyHere(id) => id,
        }
    }
}

/// The accounts the person has and the grants syncd holds, as accountd answered them together.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// Every account, in accountd's order, with what accountd calls its provider.
    pub accounts: Vec<ListedAccount>,
    /// The grants syncd holds.
    pub grants: Vec<Grant>,
}

impl Snapshot {
    /// The account `id` is.
    pub fn account(&self, id: &AccountId) -> Option<&Account> {
        self.accounts
            .iter()
            .map(|listed| &listed.account)
            .find(|account| account.id == *id)
    }

    /// What accountd calls the provider of the account `id`, when it says.
    pub fn provider_label(&self, id: &AccountId) -> Option<&str> {
        self.accounts
            .iter()
            .find(|listed| listed.account.id == *id)
            .and_then(|listed| listed.provider_label.as_deref())
    }
}

/// The folder an account's journals, cache and drive are kept under, and its drive's id.
fn drive_dir(account: &AccountId) -> Option<AccountDir> {
    AccountDir::parse(&porter_ids::object_segment(account.as_str()))
}

/// What syncd keeps of the accounts, over accountd through porter-client.
#[derive(Debug)]
pub struct Keep<T> {
    grants: Arc<Grants<T>>,
    accounts: Accounts<T>,
    /// One switch at a time: a consent sheet answers the ask that opened it.
    turning: tokio::sync::Mutex<()>,
    paths: Paths,
    hub: Hub,
    /// The drives shown in Files, let go before their account is removed.
    drives: DriveSet,
}

impl<T: GrantsLink + AccountsLink> Keep<T> {
    /// The accounts of `porter`; a removal wipes what `paths` and `hub` name.
    pub fn new(porter: &Porter<T>, paths: Paths, hub: Hub) -> Self {
        Self {
            grants: Arc::new(porter.grants()),
            accounts: porter.accounts(),
            turning: tokio::sync::Mutex::new(()),
            paths,
            hub,
            drives: DriveSet::default(),
        }
    }

    /// With the drives shown in Files: a removal lets the account's drive go first, keeping
    /// what was not uploaded in its folder.
    #[must_use]
    pub fn with_drives(mut self, drives: DriveSet) -> Self {
        self.drives = drives;
        self
    }

    /// The accounts and syncd's grants, as accountd answers now.
    ///
    /// # Errors
    /// accountd could not be asked.
    pub async fn snapshot(&self) -> Result<Snapshot, GrantsError> {
        let accounts = self.grants.accounts().await?;
        let grants = self.grants.grants().await?;
        Ok(Snapshot { accounts, grants })
    }

    /// "Add Account": accountd's add-account sheet. An account the person signs in to that
    /// accountd already had is [`Continued::AlreadyHere`].
    pub async fn add_account(&self, window: &ParentWindow) -> Result<Continued, ContinueError> {
        match self.accounts.add(ProviderHint::Any, window).await {
            Ok(id) => Ok(Continued::SignedIn(id)),
            Err(AccountsError::AlreadyAdded(id)) => Ok(Continued::AlreadyHere(id)),
            Err(AccountsError::Refused(Refusal::Dismissed)) => Err(ContinueError::Cancelled),
            Err(AccountsError::Busy) => Err(ContinueError::Busy),
            Err(AccountsError::Link(_)) => Err(ContinueError::Unreachable),
            Err(_) => Err(ContinueError::Failed),
        }
    }

    /// "Add Account", then the first thing the account offers for syncd to keep (the files, else
    /// the photos): the person asked to sync this account. The switch turns on only once the
    /// account was added.
    pub async fn add_and_keep(&self, window: &ParentWindow) -> Result<Continued, ContinueError> {
        let continued = self.add_account(window).await?;
        let account = continued.account().clone();
        let first = match self.snapshot().await {
            Ok(snapshot) => snapshot
                .account(&account)
                .and_then(|row| sync_offers(row).first().copied()),
            Err(_) => None,
        };
        if let Some(class) = first {
            // A switch that is refused leaves the account added with its switches off, as the
            // person can turn them on in the rows.
            if let Err(why) = self.set_sync(&account, class, Toggle::On).await {
                eprintln!("syncd: could not switch sync on for a new account: {why}");
            }
        }
        Ok(continued)
    }

    /// Turns syncd's keeping of `class` of `account` on or off.
    pub async fn set_sync(
        &self,
        account: &AccountId,
        class: SyncClass,
        toggle: Toggle,
    ) -> Result<(), SwitchError> {
        let snapshot = self.snapshot().await.map_err(|_| SwitchError::Unreachable)?;
        let row = snapshot
            .account(account)
            .ok_or(SwitchError::UnknownAccount)?;
        match toggle {
            Toggle::On => {
                if !sync_offers(row).contains(&class) {
                    return Err(SwitchError::NothingToKeep);
                }
                if sync_allowed(&snapshot.grants, account, class) {
                    return Ok(());
                }
                let offer = ConsentOffer {
                    need: need_for(row, class),
                    class: class.class(),
                    usage: Usage::Background,
                };
                self.ask(account, &offer).await
            }
            _ => self.revoke(&snapshot.grants, &keys_of(account, class)).await,
        }
    }

    /// Whether Files offers "Show in Files" for `account`: its drive is one the listing reads
    /// whole, and its provider asks the whole drive on use.
    pub async fn offers_files(&self, account: &Account) -> bool {
        lists_whole_drive(account)
            && self
                .grants
                .asks_on_use(&account.provider, CapabilityKind::Storage)
                .await
                .unwrap_or(false)
    }

    /// "Show in Files" on or off for `account`. On, when the account was not given the whole
    /// drive yet, asks the person for it in a sign-in of its own (the browser asks that alone),
    /// then asks for syncd's grant of it; a refusal there is [`SwitchError::NotGiven`] and nothing
    /// is recorded. Off takes the grant back; the listing's journal and the access given stay.
    pub async fn show_in_files(
        &self,
        account: &AccountId,
        toggle: Toggle,
    ) -> Result<(), SwitchError> {
        let snapshot = self.snapshot().await.map_err(|_| SwitchError::Unreachable)?;
        let row = snapshot
            .account(account)
            .ok_or(SwitchError::UnknownAccount)?;
        match toggle {
            Toggle::On => {
                if !self.offers_files(row).await {
                    return Err(SwitchError::NothingToKeep);
                }
                if drive_allowed(&snapshot.grants, account) {
                    return Ok(());
                }
                if !claims_whole_drive(row) {
                    self.grants
                        .extend_access(
                            account,
                            &[CapabilityKind::Storage],
                            &ParentWindow::Unparented,
                        )
                        .await
                        .map_err(|why| {
                            eprintln!("syncd: the whole drive was not given: {why:?}");
                            SwitchError::NotGiven
                        })?;
                }
                let offer = ConsentOffer {
                    need: crate::datasets::storage::drive_need(),
                    class: porter_core::DataClass::Files,
                    usage: Usage::Background,
                };
                self.ask(account, &offer).await
            }
            _ => self.revoke(&snapshot.grants, &[drive_key(account)]).await,
        }
    }

    /// "Keep Everything Downloaded" on or off for the drive of `account`, which must be shown in
    /// Files. It is kept in syncd's settings, so the drive keeps everything again after a restart
    /// (and after Show in Files is turned off and on again); the drive shown now follows at once.
    pub async fn keep_everything(
        &self,
        account: &AccountId,
        toggle: Toggle,
    ) -> Result<(), SwitchError> {
        let snapshot = self.snapshot().await.map_err(|_| SwitchError::Unreachable)?;
        snapshot
            .account(account)
            .ok_or(SwitchError::UnknownAccount)?;
        if !drive_allowed(&snapshot.grants, account) {
            return Err(SwitchError::NothingToKeep);
        }
        let dir = drive_dir(account).ok_or(SwitchError::UnknownAccount)?;
        self.drives
            .set_keep_all(dir.as_str(), toggle == Toggle::On)
            .map_err(|why| {
                eprintln!("syncd: whether to keep everything downloaded could not be saved: {why}");
                SwitchError::Unsaved
            })
    }

    /// Whether the drive of `account` is kept downloaded whole.
    pub fn keeps_everything(&self, account: &AccountId) -> bool {
        drive_dir(account).is_some_and(|dir| self.drives.keeps_all(dir.as_str()))
    }

    /// Whether keeping the drive of `account` downloaded whole needs more room than this computer
    /// has free.
    pub fn short_of_room(&self, account: &AccountId) -> bool {
        drive_dir(account).is_some_and(|dir| self.drives.short_of_room(dir.as_str()))
    }

    /// Hears the drives whose room to keep everything downloaded changes, by drive id
    /// ([`Keep::account_of_drive`] names the account).
    pub fn watch_room(&self) -> tokio::sync::broadcast::Receiver<String> {
        self.drives.watch_room()
    }

    /// The account whose drive is `drive` (a drive's id is its account's folder name).
    pub async fn account_of_drive(&self, drive: &str) -> Option<AccountId> {
        let snapshot = self.snapshot().await.ok()?;
        snapshot
            .accounts
            .into_iter()
            .map(|listed| listed.account.id)
            .find(|id| drive_dir(id).is_some_and(|dir| dir.as_str() == drive))
    }

    /// Asks accountd for the grant `offer` names, for `account`: accountd's chooser and consent
    /// sheet. The grant of another account, which the person may pick in the chooser, is theirs
    /// to give and stays; this switch stays off then.
    async fn ask(&self, account: &AccountId, offer: &ConsentOffer) -> Result<(), SwitchError> {
        let _one_switch = self.turning.lock().await;
        match self
            .grants
            .request_grant(offer, &ParentWindow::Unparented)
            .await
        {
            Ok(chosen) if chosen.account == *account => Ok(()),
            Ok(_) => Err(SwitchError::NotGiven),
            Err(GrantsError::Refused(_) | GrantsError::Busy) => Err(SwitchError::NotGiven),
            Err(_) => Err(SwitchError::Unreachable),
        }
    }

    /// Takes back every grant of syncd's under one of `wanted`.
    async fn revoke(&self, held: &[Grant], wanted: &[GrantKey]) -> Result<(), SwitchError> {
        for grant in held.iter().filter(|g| holds(g, wanted)) {
            if self.grants.revoke(&grant.id).await.is_err() {
                return Err(SwitchError::Unsaved);
            }
        }
        Ok(())
    }

    /// Removes `account` here: its drive is let go (held, and what was made or changed in its
    /// folder and not uploaded written out to a folder of its own beside it), then its datasets
    /// stop, its journals, mirrors and cache are deleted, and syncd's grants of it are taken
    /// back. What was kept is returned. A removal that fails leaves the account and starts its
    /// drive again; what was kept stays where it is, so trying again keeps nothing twice.
    pub async fn remove(&self, account: &AccountId) -> Result<Vec<Kept>, RemoveError> {
        let snapshot = self.snapshot().await.map_err(|_| RemoveError::Unreachable)?;
        snapshot.account(account).ok_or(RemoveError::Unknown)?;
        let dir = drive_dir(account).ok_or(RemoveError::Unknown)?;
        let failed = |why: RemoveError| {
            self.drives.release(dir.as_str());
            why
        };
        let kept = self
            .drives
            .retire(&self.paths, &dir)
            .await
            .map_err(|why| failed(RemoveError::Files(why)))?;
        removal::wipe(&self.paths, &self.hub, &dir)
            .await
            .map_err(|why| failed(RemoveError::Files(why.to_string())))?;
        for grant in snapshot.grants.iter().filter(|g| g.key.account == *account) {
            self.grants
                .revoke(&grant.id)
                .await
                .map_err(|_| failed(RemoveError::Grants))?;
        }
        self.drives.forget(dir.as_str());
        Ok(kept)
    }
}
