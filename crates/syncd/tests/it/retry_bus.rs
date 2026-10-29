//! A supervisor whose look could not be finished says so, so that its loop looks again in
//! seconds (`next_look`) and not at the next ten-minute rescan: a grant list that does not answer
//! yet on a loaded machine must not leave a granted account unmirrored for ten minutes.

use crate::common::bus::PrivateBus;
use crate::common::hosted::host;
use ferry_engine::hub::{Access, Hub};
use ferry_engine::scheduler::{Network, Settings};
use ferry_pim_mirror::{AccountdUnavailable, PimConfig, PimGrants, PimSupervisor, Wiring};
use ferry_vdir::PimKind;
use porter_core::Candidate;
use porter_fake::cloud_provider;
use porter_secrets::MemorySecrets;
use porter_service_core::registry::Registry;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use syncd::datasets::pim::SyncdSources;
use tokio::sync::watch;

/// Grants that cannot be asked while `down` is set, and are none otherwise.
struct Flaky {
    down: Arc<AtomicBool>,
}

impl PimGrants for Flaky {
    async fn granted(&self, _kind: PimKind) -> Result<Vec<Candidate>, AccountdUnavailable> {
        match self.down.load(Ordering::SeqCst) {
            true => Err(AccountdUnavailable),
            false => Ok(Vec::new()),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_look_that_could_not_ask_for_grants_says_it_was_not_whole_and_the_next_one_is() {
    let bus = PrivateBus::start();
    let hosted = host(
        cloud_provider(),
        Registry::default(),
        MemorySecrets::default(),
    );
    let root = bus.scratch().join("syncd");
    let (_network_up, network) = watch::channel(Network::Unmetered);
    let wiring = Wiring {
        accounts: hosted.accounts,
        hub: Hub::default(),
        journals: root.join("state/porter/sync"),
        mirrors: root.join("data/porter/vdir"),
        settings: Settings::quick(),
        network,
        owners: Access::default(),
        clock: ferry_sync::Clock::new(porter_ids::clock::SystemClock),
        nonce: "t1".to_owned(),
    };
    let down = Arc::new(AtomicBool::new(true));
    let mut supervisor = PimSupervisor::new(
        wiring,
        Flaky {
            down: Arc::clone(&down),
        },
        SyncdSources,
        PimConfig::default(),
    );
    assert!(
        !supervisor.tick().await,
        "the grants could not be asked: the look is not whole"
    );
    down.store(false, Ordering::SeqCst);
    assert!(supervisor.tick().await, "asked and answered: whole");
}
