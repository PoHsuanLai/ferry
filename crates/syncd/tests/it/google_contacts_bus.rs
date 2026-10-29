//! A Google account's Contacts mirrored into the vdir, on one private bus: accountd (hosted in
//! process over in-memory secrets, a fake provider that mints a token for each endpoint's
//! audience) holds the credential, which adds the bearer; syncd's supervisor, holding a Contacts
//! grant whose capability's transport is `google_api`, lists the contacts of the rig's
//! `FakeGoogle` and mirrors them as `.vcf` files under a scratch `$XDG_DATA_HOME`. Nothing here
//! touches the real session bus, `~/.local` or the network.

use crate::common;
use porter_ids::word::word as word_of;

use common::bus::PrivateBus;
use common::caller as caller_of;
use common::hosted::host;
use ferry_engine::hub::{Access, Hub};
use ferry_engine::scheduler::{Network, Settings};
use ferry_pim_mirror::{ClientGrants, PimConfig, PimSupervisor, Wiring};
use ferry_vdir::{COLOR, DISPLAYNAME, PimKind, is_complete, items_in};
use porter_callers::CallerRole;
use porter_capability::StorageScope;
use porter_capability::{CapabilityKind, Claim, Offer, Provenance, Subject};
use porter_core::{
    Account, AccountLabel, AccountState, AuthKind, Credential, EndpointUrl, Family, LoginName,
    Restriction, SecretKey, SecretPurpose, SecretText, ServiceEndpoint, SpaceScope, Tls,
};
use porter_core::{AccountId, GrantId};
use porter_core::{
    GrantKey,
    consent::{Decision, Grant, GrantScope, Usage},
};
use porter_fake::FakeProvider;
use porter_fake_servers::{FakeGoogle, FakeIssuer, GoogleHandle, IssuerHandle, Running};
use porter_ids::UnixSeconds;
use porter_provider::Provider;
use porter_secrets::{MemorySecrets, Secrets};
use porter_service_core::registry::Registry;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use syncd::datasets::contacts::ContactSources;
use syncd::keep::sync_app as syncd_app;
use syncd::paths::Paths;
use tokio::sync::watch;

const ACCOUNT: &str = "g-pim";
const SEGMENT: &str = "g_pim";
const PROVIDER: &str = r#"
id = "fake-google"
label = "Fake Google"
mark = "generic"
[auth]
kind = "oauth_pkce"
issuer = "google"
[discovery]
kind = "autoconfig"
[[capability]]
family = "google_people"
kind = "contacts"
v = { access = "read_write", delta = "poll", transport = "google_api", collections = "absent" }
"#;

/// What the fake provider mints for the audience of this account's endpoint of `family`.
fn bearer(family: Family) -> String {
    format!("fake:{ACCOUNT}:{}", word_of(&family))
}

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

struct Rig {
    _bus: PrivateBus,
    _issuer: Running<IssuerHandle>,
    google: Running<GoogleHandle>,
    hub: Hub,
    mirrors: PathBuf,
    supervisor: tokio::task::JoinHandle<()>,
    _network: watch::Sender<Network>,
}

impl Rig {
    /// What the daemon has done so far, as [`common::pulse`] counts it.
    fn pulse(&self) -> u64 {
        common::pulse(&self.hub, self.google.hits().len())
    }

    fn account_dir(&self) -> PathBuf {
        self.mirrors.join(SEGMENT)
    }

    /// Every collection directory of the account.
    fn dirs(&self) -> Vec<PathBuf> {
        std::fs::read_dir(self.account_dir())
            .map(|entries| entries.filter_map(Result::ok).map(|e| e.path()).collect())
            .unwrap_or_default()
    }

    /// The directory of the address book the server calls `name` (the directory is named by a
    /// hash of the id, so a test finds it by its `displayname` file).
    fn book(&self, name: &str) -> Option<PathBuf> {
        self.dirs()
            .into_iter()
            .find(|dir| read(&dir.join(DISPLAYNAME)).as_deref() == Some(name))
    }

    fn items(&self, name: &str) -> Vec<String> {
        self.book(name)
            .map(|dir| items_in(&dir, PimKind::Contacts))
            .unwrap_or_default()
    }

    /// The text of the contact of the address book `name` that holds `needle`.
    fn item_with(&self, name: &str, needle: &str) -> Option<String> {
        let dir = self.book(name)?;
        items_in(&dir, PimKind::Contacts)
            .into_iter()
            .filter_map(|file| read(&dir.join(file)))
            .map(|text| text.replace("\r\n ", ""))
            .find(|text| text.contains(needle))
    }

    fn hits_under(&self, prefix: &str) -> Vec<porter_fake_servers::Hit> {
        self.google
            .hits()
            .into_iter()
            .filter(|h| h.target.starts_with(prefix))
            .collect()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.supervisor.abort();
    }
}

/// Polls every 20 ms; the limit is idle time (`common::eventually_alive`): the wait ends only when
/// the rig has not begun a cycle or served a request for [`test_kit::GENEROUS`].
async fn eventually(rig: &Rig, what: &str, check: impl FnMut() -> bool) {
    common::eventually_alive(what, || rig.pulse(), check).await;
}

/// A Google account granted to syncd for its contacts, over a fake Google holding the seeded set.
async fn rig() -> Rig {
    let issuer = FakeIssuer::start().await.expect("issuer");
    let google = FakeGoogle::start(&issuer).await.expect("google");
    google.accept_bearer(&bearer(Family::GooglePeople));
    google.seed_people();
    let base = google.base_url().to_owned();
    let endpoint = |family, path: &str| ServiceEndpoint {
        family,
        url: EndpointUrl::parse(&format!("{base}{path}")).expect("url"),
        tls: Tls::Plain,
        login: LoginName("ada@gmail.test".into()),
    };
    let provider = FakeProvider::from_file(PROVIDER);
    let account = Account {
        id: AccountId::parse(ACCOUNT).expect("id"),
        provider: provider.spec().id.clone(),
        label: AccountLabel("ada@gmail.test".into()),
        state: AccountState::Ok,
        auth: AuthKind::OAuthPkce,
        capabilities: provider
            .spec()
            .capabilities
            .iter()
            .map(|row| Claim {
                subject: Subject::Account,
                offer: Offer::Present(row.capability.clone()),
                provenance: Provenance::Declared,
            })
            .collect(),
        restriction: Restriction::none(),
        endpoints: vec![endpoint(Family::GooglePeople, "/v1")],
    };
    let grants = vec![Grant {
        id: GrantId::parse("g-pim-contacts").expect("grant"),
        key: GrantKey {
            app: syncd_app(),
            account: account.id.clone(),
            kind: CapabilityKind::Contacts,
            class: PimKind::Contacts.class(),
            usage: Usage::Background,
            space: SpaceScope::Any,
            reach: StorageScope::AppFolder,
        },
        decision: Decision::Allow,
        scope: GrantScope::Always,
        at: UnixSeconds(1),
    }];
    let secrets = MemorySecrets::default();
    secrets
        .put(
            &SecretKey {
                account: account.id.clone(),
                purpose: SecretPurpose::OAuthRefresh,
            },
            &Credential::OAuth {
                access: SecretText::new("SECRET-ACCESS-TOKEN"),
                refresh: SecretText::new("SECRET-REFRESH-TOKEN"),
                expires_at: UnixSeconds(1),
                granted: Default::default(),
            },
        )
        .await
        .expect("secret");

    let bus = PrivateBus::start();
    let hosted = host(
        provider,
        Registry {
            grants,
            accounts: vec![account],
            toggles: vec![],
        },
        secrets,
    );
    let accounts = hosted.accounts;
    let root = bus.scratch().join("syncd");
    let paths = Paths {
        journals: root.join("state/porter/sync"),
        mirrors: root.join("data/porter/vdir"),
        callers_system: root.join("none"),
        callers_user: root.join("none"),
        cloud: root.join("Cloud"),
        drive_cache: root.join("cache/porter/drives"),
        settings: root.join("config/porter/syncd/syncd.toml"),
    };
    let hub = Hub::default();
    let (network_up, network) = watch::channel(Network::Unmetered);
    let wiring = Wiring {
        accounts: Arc::clone(&accounts),
        hub: hub.clone(),
        journals: paths.journals.clone(),
        mirrors: paths.mirrors.clone(),
        settings: Settings::quick(),
        network,
        owners: Access::default(),
        clock: ferry_sync::Clock::new(porter_ids::clock::SystemClock),
        nonce: "t1".to_owned(),
    };
    let supervisor = PimSupervisor::new(
        wiring,
        ClientGrants::new(Arc::clone(&accounts)),
        ContactSources,
        PimConfig {
            rescan: Duration::from_millis(300),
        },
    )
    .spawn();
    Rig {
        _bus: bus,
        _issuer: issuer,
        google,
        hub,
        mirrors: paths.mirrors,
        supervisor,
        _network: network_up,
    }
}

/// Whether a path is one contact's read (`people.get`) rather than a listing.
fn is_single_fetch(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let after = |name: &str| {
        parts
            .iter()
            .position(|p| *p == name)
            .map(|at| &parts[at + 1..])
    };
    after("people").is_some_and(|rest| rest.len() == 1 && rest[0] != "me")
}

/// Everything the seeded fake holds is mirrored.
fn everything_mirrored(rig: &Rig) -> bool {
    rig.items("Contacts").len() == 2
}

#[tokio::test(flavor = "multi_thread")]
async fn the_contacts_mirror_into_the_vdir() {
    let rig = rig().await;
    eventually(&rig, "the address book is mirrored", || {
        everything_mirrored(&rig)
    })
    .await;

    // One collection called Contacts, whole vCards, in a directory told apart by its kind.
    let book = rig.book("Contacts").expect("book");
    assert!(read(&book.join(COLOR)).is_none());
    let dirs: Vec<String> = rig
        .dirs()
        .iter()
        .filter_map(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    assert_eq!(dirs, ["google-contacts"]);
    let cards = items_in(&book, PimKind::Contacts);
    assert_eq!(cards, ["people_c1001.vcf", "people_c1002.vcf"]);
    let ada = read(&book.join("people_c1001.vcf")).expect("ada");
    assert!(is_complete(PimKind::Contacts, ada.as_bytes()), "{ada}");
    for want in [
        "VERSION:3.0\r\n",
        "UID:people/c1001\r\n",
        "FN:Ada Lovelace\r\n",
        "EMAIL;TYPE=INTERNET,HOME:ada@example.test\r\n",
        "TEL;TYPE=CELL:+44 20 7946 0000\r\n",
        "ORG:Analytical Engines\r\n",
        "BDAY:1815-12-10\r\n",
    ] {
        assert!(ada.contains(want), "{want} in {ada}");
    }

    // Sync1 names it <account>/<dataset>.
    let settings = caller_of("org.quire.Settings", CallerRole::Settings);
    assert_eq!(
        rig.hub.names_for(&settings),
        [format!("{SEGMENT}/pim_card_google_contacts")]
    );

    // The relay, not syncd, authenticated with the People audience's token; nothing was written
    // to the server; no contact was fetched singly (the pages carry them); no other API was
    // asked: calendars and task lists are callie's.
    let hits = rig.google.hits();
    let under = rig.hits_under("/v1/people/");
    assert!(!under.is_empty(), "{under:?}");
    let want = format!("Bearer {}", bearer(Family::GooglePeople));
    assert!(
        under
            .iter()
            .all(|h| h.authorization.as_deref() == Some(want.as_str()) && h.status == 200),
        "{under:?}"
    );
    assert_eq!(
        under.len(),
        hits.len(),
        "{:?}",
        hits.iter().map(|h| &h.target).collect::<Vec<_>>()
    );
    assert!(hits.iter().all(|h| h.method == "GET"));
    assert!(
        hits.iter()
            .all(|h| !is_single_fetch(h.target.split('?').next().unwrap_or(""))),
        "{:?}",
        hits.iter().map(|h| &h.target).collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_changes_removals_and_new_contacts_arrive_at_the_next_poll() {
    let rig = rig().await;
    eventually(&rig, "the first mirror", || everything_mirrored(&rig)).await;

    // A changed one, a removed one, a new one.
    rig.google.put_person(
        "people/c1001",
        json!({"names": [{"displayName": "Ada King"}], "emailAddresses": [{"value": "ada@new.test"}]}),
    );
    rig.google.remove_person("people/c1002");
    rig.google
        .put_person("people/c1003", json!({"names": [{"displayName": "Linus"}]}));
    eventually(&rig, "changes, removals and new contacts arrive", || {
        rig.item_with("Contacts", "FN:Ada King").is_some()
            && rig.items("Contacts") == ["people_c1001.vcf", "people_c1003.vcf"]
    })
    .await;
    let settings = caller_of("org.quire.Settings", CallerRole::Settings);
    assert_eq!(rig.hub.names_for(&settings).len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn local_edits_deletions_and_new_files_are_never_sent_and_a_remote_change_overwrites() {
    let rig = rig().await;
    eventually(&rig, "the first mirror", || everything_mirrored(&rig)).await;
    let book = rig.book("Contacts").expect("book");
    std::fs::write(book.join("people_c1002.vcf"), "locally edited").expect("edit card");
    std::fs::remove_file(book.join("people_c1001.vcf")).expect("delete");
    std::fs::write(book.join("mine.vcf"), "BEGIN:VCARD\r\nEND:VCARD\r\n").expect("new");
    // A cycle that saw the edits has passed.
    common::a_cycle_of_each(&rig.hub).await;
    let writes: Vec<_> = rig
        .google
        .hits()
        .into_iter()
        .filter(|h| h.method != "GET")
        .collect();
    assert!(writes.is_empty(), "{writes:?}");
    assert_eq!(
        read(&book.join("people_c1002.vcf")).as_deref(),
        Some("locally edited")
    );
    assert!(!book.join("people_c1001.vcf").exists());

    // The server's next change to each contact replaces the local state (no conflict).
    rig.google.put_person(
        "people/c1002",
        json!({"names": [{"displayName": "Grace B. Hopper"}]}),
    );
    rig.google.put_person(
        "people/c1001",
        json!({"names": [{"displayName": "Ada King"}]}),
    );
    eventually(&rig, "the server's state replaces the local one", || {
        read(&book.join("people_c1002.vcf")).is_some_and(|t| t.contains("Grace B. Hopper"))
            && read(&book.join("people_c1001.vcf")).is_some_and(|t| t.contains("Ada King"))
    })
    .await;
    assert!(book.join("mine.vcf").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_expired_sync_token_lists_again_and_keeps_every_contact() {
    let rig = rig().await;
    eventually(&rig, "the first mirror", || everything_mirrored(&rig)).await;
    common::a_cycle_of_each(&rig.hub).await;
    let before = rig.google.hits().len();
    rig.google.put_person(
        "people/c1004",
        json!({"names": [{"displayName": "Extra Person"}]}),
    );
    rig.google.expire_sync_tokens();
    eventually(
        &rig,
        "the new contact arrives after the listing starts over",
        || rig.items("Contacts").len() == 3,
    )
    .await;
    // The source meets the refusal (400) at its next poll. The new contact can arrive before it,
    // so the refusal is waited for, not counted once the contact is there.
    eventually(&rig, "the contacts met the expiry", || {
        rig.google
            .hits()
            .split_off(before)
            .iter()
            .any(|h| h.status == 400 && h.target.starts_with("/v1/people/me/connections"))
    })
    .await;
    // Nothing was lost.
    assert_eq!(rig.items("Contacts").len(), 3);
}
