//! The supervisor: every `rescan` it asks accountd which grants of the daemon's kinds the daemon
//! holds and keeps one [`AccountMirrors`] per granted account and kind. A new grant starts
//! mirroring; a grant that is gone (revoked, or the account removed) stops it and deletes the
//! account's mirror. An accountd that cannot be asked changes nothing.
//!
//! What differs between the daemons that run it is [`Sources`]: which kinds they mirror and which
//! source reads a grant.

use crate::catalog::Catalog;
use crate::grants::PimGrants;
use crate::mirrors::{AccountMirrors, RefreshError, Wiring};
use ferry_engine::account::AccountDir;
use ferry_engine::scheduler::next_look;
use ferry_vdir::PimKind;
use porter_client::GrantsLink;
use porter_core::Candidate;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::time::Duration;
use tokio::task::JoinHandle;

/// What a daemon decides: the kinds it mirrors and, for a grant, the source that reads it.
pub trait Sources<T: GrantsLink + 'static>: Send + Sync + 'static {
    /// The daemon's name, which its log lines begin with.
    const NAME: &'static str;

    /// The kinds the daemon mirrors, one grant each.
    const KINDS: &'static [PimKind];

    /// Reads the collections of `mirrors.kind()` that `candidate`'s grant reaches, through the
    /// source its transport names (never its provider), and brings `mirrors` to match
    /// ([`AccountMirrors::refresh_with`]).
    fn refresh(
        &self,
        mirrors: &mut AccountMirrors,
        wiring: &Wiring<T>,
        candidate: &Candidate,
    ) -> impl Future<Output = Result<(), RefreshError>> + Send;
}

/// How the supervisor behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PimConfig {
    /// How often grants and collections are read again.
    pub rescan: Duration,
}

impl Default for PimConfig {
    fn default() -> Self {
        Self {
            rescan: Duration::from_secs(600),
        }
    }
}

/// Keeps the mirrors of every granted account.
#[derive(Debug)]
pub struct PimSupervisor<T, G, S> {
    wiring: Wiring<T>,
    grants: G,
    sources: S,
    config: PimConfig,
    accounts: BTreeMap<(AccountDir, PimKind), AccountMirrors>,
    silent: BTreeSet<PimKind>,
    catalog: Catalog,
}

impl<T, G, S> PimSupervisor<T, G, S>
where
    T: GrantsLink + 'static,
    G: PimGrants + 'static,
    S: Sources<T>,
{
    /// A supervisor over `wiring`, asking `grants`, reading through `sources`.
    pub fn new(wiring: Wiring<T>, grants: G, sources: S, config: PimConfig) -> Self {
        Self {
            wiring,
            grants,
            sources,
            config,
            accounts: BTreeMap::new(),
            silent: BTreeSet::new(),
            catalog: Catalog::default(),
        }
    }

    /// The collections running, updated at the end of every look.
    pub fn catalog(&self) -> Catalog {
        self.catalog.clone()
    }

    /// One look: grants, collections, what to start and what to stop. `true` when every step
    /// went through; `false` when accountd could not be asked or a collection could not be
    /// mirrored (the next look is then soon, see [`next_look`]).
    pub async fn tick(&mut self) -> bool {
        let mut whole = true;
        for kind in S::KINDS.iter().copied() {
            let Ok(candidates) = self.grants.granted(kind).await else {
                eprintln!(
                    "{}: cannot ask for the {kind:?} grants; asking again soon",
                    S::NAME
                );
                whole = false;
                continue;
            };
            if candidates.is_empty() && self.silent.insert(kind) {
                eprintln!("{}: no {kind:?} grant, so no {kind:?} is mirrored", S::NAME);
            }
            let mut granted: Vec<AccountDir> = Vec::new();
            for candidate in &candidates {
                let Some(account) =
                    AccountDir::parse(&porter_ids::object_segment(candidate.account.as_str()))
                else {
                    continue;
                };
                granted.push(account.clone());
                let mirrors = self
                    .accounts
                    .entry((account.clone(), kind))
                    .or_insert_with(|| AccountMirrors::new(account.clone(), kind));
                if let Err(why) = self.sources.refresh(mirrors, &self.wiring, candidate).await {
                    eprintln!(
                        "{}: cannot mirror {kind:?} of {account}: {why}; trying again soon",
                        S::NAME
                    );
                    whole = false;
                }
            }
            let stale: Vec<(AccountDir, PimKind)> = self
                .accounts
                .keys()
                .filter(|(account, k)| *k == kind && !granted.contains(account))
                .cloned()
                .collect();
            for key in stale {
                if let Some(mut mirrors) = self.accounts.remove(&key) {
                    mirrors.retire_all(&self.wiring).await;
                }
            }
        }
        self.catalog.replace(
            self.accounts
                .values()
                .flat_map(AccountMirrors::collections)
                .collect(),
        );
        whole
    }

    /// Runs `tick` every `rescan`, for as long as the task lives; after a look that could not
    /// do all it meant to, again soon and then less often (a passing error is never a stop
    /// until the next rescan).
    pub fn spawn(mut self) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut failures = 0u32;
            loop {
                failures = if self.tick().await {
                    0
                } else {
                    failures.saturating_add(1)
                };
                tokio::time::sleep(next_look(self.config.rescan, failures)).await;
            }
        })
    }
}
