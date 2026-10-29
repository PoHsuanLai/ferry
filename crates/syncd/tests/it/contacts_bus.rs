//! The PIM mirror end to end on one private bus: accountd (hosted in process over in-memory
//! secrets) holds the app password and runs the relay; syncd's supervisor, holding a Contacts
//! grant, discovers the address books of a fake Nextcloud through the relay and mirrors each into
//! a vdir under a scratch `$XDG_DATA_HOME`. Nothing here touches the real session bus,
//! `~/.local` or the network.

use crate::common;

use common::bus::PrivateBus;
use common::caller as caller_of;
use common::hosted::{Link, Service, host, keep};
use ferry_engine::hub::{Access, Hub};
use ferry_engine::scheduler::{Network, Settings};
use ferry_pim_mirror::{
    AccountdUnavailable, ClientGrants, PimConfig, PimGrants, PimSupervisor, Wiring,
};
use ferry_vdir::{COLOR, DISPLAYNAME, PimKind, is_complete, items_in};
use porter_callers::{Caller, CallerRole};
use porter_capability::StorageScope;
use porter_capability::{CapabilityKind, Claim, Offer, Provenance, Subject};
use porter_core::{
    Account, AccountLabel, AccountState, AuthKind, Credential, DataClass, EndpointUrl, Family,
    LoginName, Restriction, SecretKey, SecretPurpose, SecretText, ServiceEndpoint, SpaceScope, Tls,
};
use porter_core::{AccountId, GrantId, ProviderId};
use porter_core::{
    GrantKey,
    consent::{Decision, Grant, GrantScope, Usage},
};
use porter_fake::FakeProvider;
use porter_fake_servers::{FakeNextcloud, NextcloudHandle, Running};
use porter_ids::UnixSeconds;
use porter_provider::Provider;
use porter_secrets::{MemorySecrets, Secrets};
use porter_service_core::registry::Registry;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use syncd::datasets::contacts::ContactSources;
use syncd::keep::{Keep, sync_app as syncd_app};
use syncd::paths::Paths;
use tokio::sync::watch;

/// How many kinds a look asks the accounts about: syncd mirrors address books.
const KINDS: usize = 1;

const PASSWORD: &str = "S3CRET-PIM-APP-PASSWORD";
const ACCOUNT: &str = "pim-cloud";
const SEGMENT: &str = "pim_cloud";
const PROVIDER: &str = r#"
id = "pim-cloud"
label = "PIM Cloud"
mark = "generic"
[auth]
kind = "app_password"
[discovery]
kind = "fixed"
[[capability]]
family = "carddav"
kind = "contacts"
v = { access = "read_write", delta = "poll", transport = "carddav", collections = "present" }
"#;

fn card(uid: &str, name: &str) -> String {
    format!("BEGIN:VCARD\r\nVERSION:3.0\r\nUID:{uid}\r\nFN:{name}\r\nEND:VCARD\r\n")
}

/// Polls every 20 ms; the limit is idle time (`common::eventually_alive`): the wait ends only when
/// the rig has not begun a cycle or served a request for [`test_kit::GENEROUS`] (a poll cycle
/// is a second here), so a loaded machine that mirrors slowly is never cut short.
async fn eventually(rig: &Rig, what: &str, check: impl FnMut() -> bool) {
    common::eventually_alive(what, || rig.pulse(), check).await;
}

/// Polls every 20 ms until `done` items of `total` are finished. The wait is idle time, not
/// wall-clock time: a round of twenty large items takes minutes on a loaded machine, and a
/// daemon that keeps mirroring is working, so the wait fails only when the count has not grown
/// for [`test_kit::GENEROUS`].
async fn eventually_progressing(what: &str, mut done: impl FnMut() -> usize, total: usize) {
    let mut deadline = test_kit::Deadline::idle(test_kit::GENEROUS);
    let mut best = 0;
    loop {
        let now = done();
        if now == total {
            return;
        }
        if now > best {
            best = now;
            deadline.progress();
        }
        if deadline.passed() {
            deadline.fail(what);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Tells a blocking helper to stop when the test ends, by success or by panic: the runtime waits
/// for its blocking threads when it drops, so a helper still looping after a panic hangs the test
/// instead of failing it.
struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

fn read(path: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// `accounts.grants` as the supervisor sees them, which a test can make unanswering, and
/// which counts the questions it is asked (a look asks once per kind).
struct Switch {
    inner: ClientGrants<Link>,
    up: Arc<AtomicBool>,
    asked: Arc<AtomicUsize>,
}

impl PimGrants for Switch {
    async fn granted(
        &self,
        kind: PimKind,
    ) -> Result<Vec<porter_core::Candidate>, AccountdUnavailable> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        match self.up.load(Ordering::Relaxed) {
            true => self.inner.granted(kind).await,
            false => Err(AccountdUnavailable),
        }
    }
}

struct Rig {
    bus: PrivateBus,
    nextcloud: Running<NextcloudHandle>,
    hub: Hub,
    mirrors: PathBuf,
    service: Arc<Service>,
    /// syncd's accounts, whose `remove` is what the Settings row runs.
    own: Keep<Link>,
    accountd_up: Arc<AtomicBool>,
    asked: Arc<AtomicUsize>,
    grants: Vec<GrantId>,
    supervisor: tokio::task::JoinHandle<()>,
    _network: watch::Sender<Network>,
}

impl Rig {
    /// Waits until the supervisor has made two whole looks that began after this call: a look
    /// asks once per kind, so the look in flight may have asked some already, the next is whole,
    /// and one more question shows it has ended. It waits for looks, not for a time, so a
    /// loaded machine that looks late is waited for, and an idle one is not kept waiting.
    async fn after_two_looks(&self) {
        let wanted = self.asked.load(Ordering::SeqCst) + 2 * KINDS + 1;
        eventually(self, "the supervisor looks twice", || {
            self.asked.load(Ordering::SeqCst) >= wanted
        })
        .await;
    }

    /// What the daemon has done so far, as [`common::pulse`] counts it, with the questions the
    /// supervisor asked of the accounts.
    fn pulse(&self) -> u64 {
        common::pulse(
            &self.hub,
            self.nextcloud.hits().len() + self.asked.load(Ordering::SeqCst),
        )
    }

    fn account_dir(&self) -> PathBuf {
        self.mirrors.join(SEGMENT)
    }

    fn collection(&self, name: &str) -> PathBuf {
        self.account_dir().join(name)
    }

    fn requests(&self, method: &str) -> usize {
        self.nextcloud
            .hits()
            .iter()
            .filter(|h| h.method == method && h.target.contains("/addressbooks/"))
            .count()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.supervisor.abort();
    }
}

fn grant_for(kind: CapabilityKind, class: DataClass, id: &str) -> Grant {
    Grant {
        id: GrantId::parse(id).expect("grant"),
        key: GrantKey {
            app: syncd_app(),
            account: AccountId::parse(ACCOUNT).expect("account"),
            kind,
            class,
            usage: Usage::Background,
            space: SpaceScope::Any,
            reach: StorageScope::AppFolder,
        },
        decision: Decision::Allow,
        scope: GrantScope::Always,
        at: UnixSeconds(1),
    }
}

async fn rig(with_grants: bool) -> Rig {
    let nextcloud = FakeNextcloud::start("alice").await.expect("nextcloud");
    nextcloud.seed_app_password(PASSWORD);
    nextcloud.put_item("contacts", "c1.vcf", &card("c1", "Ada Lovelace"));
    nextcloud.put_item("contacts", "c2.vcf", &card("c2", "Grace Hopper"));
    let dav = format!("{}/remote.php/dav/", nextcloud.base_url());

    let spec = FakeProvider::from_file(PROVIDER);
    let endpoint = |family| ServiceEndpoint {
        family,
        url: EndpointUrl::parse(&dav).expect("url"),
        tls: Tls::Plain,
        login: LoginName("alice".into()),
    };
    let account = Account {
        id: AccountId::parse(ACCOUNT).expect("id"),
        provider: ProviderId::parse(ACCOUNT).expect("provider"),
        label: AccountLabel("alice@cloud.invalid".into()),
        state: AccountState::Ok,
        auth: AuthKind::AppPassword,
        capabilities: spec
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
        endpoints: vec![endpoint(Family::CardDav)],
    };
    let granted = [grant_for(
        CapabilityKind::Contacts,
        DataClass::Contacts,
        "pim-card",
    )];
    let grants: Vec<GrantId> = granted.iter().map(|g| g.id.clone()).collect();
    let secrets = MemorySecrets::default();
    secrets
        .put(
            &SecretKey {
                account: AccountId::parse(ACCOUNT).expect("id"),
                purpose: SecretPurpose::Password,
            },
            &Credential::Password(SecretText::new(PASSWORD)),
        )
        .await
        .expect("secret");

    let bus = PrivateBus::start();
    let hosted = host(
        spec,
        Registry {
            grants: if with_grants {
                granted.to_vec()
            } else {
                vec![]
            },
            accounts: vec![account],
            toggles: vec![],
        },
        secrets,
    );
    let accounts = Arc::clone(&hosted.accounts);
    let service = Arc::clone(&hosted.service);

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
    let own = keep(&hosted, paths.clone(), hub.clone());
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
    let accountd_up = Arc::new(AtomicBool::new(true));
    let asked = Arc::new(AtomicUsize::new(0));
    let supervisor = PimSupervisor::new(
        wiring,
        Switch {
            inner: ClientGrants::new(Arc::clone(&accounts)),
            up: Arc::clone(&accountd_up),
            asked: Arc::clone(&asked),
        },
        ContactSources,
        PimConfig {
            rescan: Duration::from_millis(300),
        },
    )
    .spawn();
    Rig {
        bus,
        nextcloud,
        hub,
        mirrors: paths.mirrors,
        service,
        own,
        accountd_up,
        asked,
        grants,
        supervisor,
        _network: network_up,
    }
}

fn settings_caller() -> Caller {
    caller_of("org.quire.Settings", CallerRole::Settings)
}

fn names(rig: &Rig) -> Vec<String> {
    let mut all = rig.hub.names_for(&settings_caller());
    all.sort();
    all
}

#[tokio::test(flavor = "multi_thread")]
async fn an_address_book_mirrors_into_the_vdir_with_its_metadata_and_nothing_else_is_asked_for() {
    let rig = rig(true).await;
    let contacts = rig.collection("contacts-contacts");
    eventually(&rig, "every item is mirrored", || {
        items_in(&contacts, PimKind::Contacts) == ["c1.vcf", "c2.vcf"]
    })
    .await;

    // The files are the server's, byte for byte, named by UID.
    assert_eq!(
        read(&contacts.join("c1.vcf")),
        Some(card("c1", "Ada Lovelace"))
    );
    assert_eq!(
        read(&contacts.join("c2.vcf")),
        Some(card("c2", "Grace Hopper"))
    );
    // The metadata is vdirsyncer's file; an address book has no colour.
    eventually(&rig, "the metadata is written", || {
        read(&contacts.join(DISPLAYNAME)).as_deref() == Some("contacts")
    })
    .await;
    assert!(!contacts.join(COLOR).exists());
    let collections: Vec<String> = std::fs::read_dir(rig.account_dir())
        .expect("account dir")
        .map(|e| e.expect("e").file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(collections, ["contacts-contacts"]);

    // Sync1 names it <account>/<dataset>.
    assert_eq!(
        names(&rig),
        [format!("{SEGMENT}/pim_card_contacts_contacts")]
    );

    // The relay, not syncd, authenticated: every DAV request carries the app password's Basic
    // header, which syncd never held.
    use base64::Engine as _;
    let want = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("alice:{PASSWORD}"))
    );
    let dav: Vec<_> = rig
        .nextcloud
        .hits()
        .into_iter()
        .filter(|h| h.target.starts_with("/remote.php/dav"))
        .collect();
    assert!(dav.len() > 3);
    assert!(
        dav.iter()
            .all(|h| h.authorization.as_deref() == Some(want.as_str()))
    );
    // Read-only: nothing was written to the server.
    assert!(
        dav.iter()
            .all(|h| !matches!(h.method.as_str(), "PUT" | "DELETE" | "MKCOL")),
        "{:?}",
        dav.iter()
            .map(|h| (&h.method, &h.target))
            .collect::<Vec<_>>()
    );
    // Calendars are callie's: syncd never asked for one.
    assert!(
        dav.iter().all(|h| !h.target.contains("/calendars/")),
        "{:?}",
        dav.iter().map(|h| &h.target).collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_edit_a_delete_a_new_item_a_rename_and_collection_changes_all_arrive() {
    let rig = rig(true).await;
    let contacts = rig.collection("contacts-contacts");
    eventually(&rig, "the first mirror", || {
        items_in(&contacts, PimKind::Contacts) == ["c1.vcf", "c2.vcf"]
    })
    .await;

    rig.nextcloud
        .put_item("contacts", "c1.vcf", &card("c1", "Ada King"));
    rig.nextcloud.delete_item("contacts", "c2.vcf");
    // A file name that is not its UID: the mirror names it by the UID.
    rig.nextcloud
        .put_item("contacts", "x-server-name.vcf", &card("real-uid", "Linus"));
    eventually(&rig, "edit, delete and new item arrive", || {
        read(&contacts.join("c1.vcf")) == Some(card("c1", "Ada King"))
            && items_in(&contacts, PimKind::Contacts) == ["c1.vcf", "real-uid.vcf"]
    })
    .await;
    assert_eq!(
        read(&contacts.join("real-uid.vcf")),
        Some(card("real-uid", "Linus"))
    );

    // Renamed on the server; a new address book; the first one deleted.
    rig.nextcloud
        .set_collection_meta("contacts", "Friends", None);
    rig.nextcloud.add_addressbook("family", "Family");
    rig.nextcloud
        .put_book_item("family", "f1.vcf", &card("f1", "Katherine Johnson"));
    let family = rig.collection("family-contacts");
    eventually(&rig, "the rename and the new address book arrive", || {
        read(&contacts.join(DISPLAYNAME)).as_deref() == Some("Friends")
            && items_in(&family, PimKind::Contacts) == ["f1.vcf"]
            && read(&family.join(DISPLAYNAME)).as_deref() == Some("Family")
    })
    .await;
    rig.nextcloud.delete_collection("contacts");
    eventually(&rig, "the removal arrives", || !contacts.exists()).await;
    assert!(
        !names(&rig)
            .iter()
            .any(|n| n.ends_with("pim_card_contacts_contacts")),
        "{:?}",
        names(&rig)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_expired_sync_token_lists_again_and_downloads_nothing_it_already_has() {
    let rig = rig(true).await;
    let contacts = rig.collection("contacts-contacts");
    eventually(&rig, "the first mirror", || {
        items_in(&contacts, PimKind::Contacts).len() == 2
    })
    .await;
    // Let a cycle after the first settle, then count what was downloaded so far.
    common::a_cycle_of_each(&rig.hub).await;
    let gets = rig.requests("GET");
    assert_eq!(gets, 2, "each contact exactly once");

    // A change moves the server's counter past every token the mirror holds; expiring tokens
    // right after it makes it stale (a token as new as the counter would stay good).
    rig.nextcloud
        .put_item("contacts", "c3.vcf", &card("c3", "Alan Turing"));
    rig.nextcloud.expire_sync_tokens();
    eventually(&rig, "the new item arrives", || {
        contacts.join("c3.vcf").exists()
    })
    .await;
    // The collection was listed again (a REPORT with no token), and only the new item came: a
    // cycle of each after the item arrived has passed.
    common::a_cycle_of_each(&rig.hub).await;
    assert_eq!(
        rig.requests("GET"),
        gets + 1,
        "no unchanged item was downloaded again"
    );
    let reports = rig
        .nextcloud
        .hits()
        .iter()
        .filter(|h| h.method == "REPORT" && h.status == 403)
        .count();
    assert!(reports >= 1, "the collection met the expiry: {reports}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_local_edit_a_deletion_and_a_new_file_are_never_sent_to_the_server() {
    let rig = rig(true).await;
    let contacts = rig.collection("contacts-contacts");
    eventually(&rig, "the first mirror", || {
        items_in(&contacts, PimKind::Contacts) == ["c1.vcf", "c2.vcf"]
    })
    .await;
    std::fs::write(contacts.join("c1.vcf"), "locally edited").expect("edit");
    std::fs::remove_file(contacts.join("c2.vcf")).expect("delete");
    std::fs::write(contacts.join("mine.vcf"), card("mine", "Local only")).expect("new file");
    // A cycle that saw the edits has passed.
    common::a_cycle_of_each(&rig.hub).await;
    let writes: Vec<_> = rig
        .nextcloud
        .hits()
        .into_iter()
        .filter(|h| matches!(h.method.as_str(), "PUT" | "DELETE" | "MKCOL"))
        .collect();
    assert!(writes.is_empty(), "{writes:?}");
    // Pull-only: the local state stays until the server next changes those items, which then
    // overwrite it (no conflict); the stray file is left alone and never uploaded.
    assert_eq!(
        read(&contacts.join("c1.vcf")).as_deref(),
        Some("locally edited")
    );
    assert!(!contacts.join("c2.vcf").exists());
    rig.nextcloud
        .put_item("contacts", "c1.vcf", &card("c1", "Ada King"));
    rig.nextcloud
        .put_item("contacts", "c2.vcf", &card("c2", "Grace B. Hopper"));
    eventually(&rig, "the server's state replaces the local one", || {
        read(&contacts.join("c1.vcf")) == Some(card("c1", "Ada King"))
            && read(&contacts.join("c2.vcf")) == Some(card("c2", "Grace B. Hopper"))
    })
    .await;
    assert!(contacts.join("mine.vcf").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reader_never_sees_a_file_that_is_not_a_complete_item() {
    let rig = rig(true).await;
    let contacts = rig.collection("contacts-contacts");
    let big = |round: u32, n: u32| {
        // Large enough to cross many pages (a plain write of it can be seen half done), no
        // larger: the other two providers' versions of this proof use about 30 KB.
        let pad = "X-PAD:0123456789abcdef0123456789abcdef0123456789abcdef\r\n".repeat(1000);
        format!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:big{n}\r\nFN:round-{round}\r\n{pad}END:VCARD\r\n"
        )
    };
    let stop = Arc::new(AtomicBool::new(false));
    let _stop_on_panic = StopOnDrop(Arc::clone(&stop));
    let reader = {
        let (dir, stop) = (contacts.clone(), Arc::clone(&stop));
        tokio::task::spawn_blocking(move || {
            let (mut reads, mut torn) = (0_u64, Vec::new());
            while !stop.load(Ordering::Relaxed) {
                for name in items_in(&dir, PimKind::Contacts) {
                    if let Ok(bytes) = std::fs::read(dir.join(&name)) {
                        reads += 1;
                        if !is_complete(PimKind::Contacts, &bytes) {
                            torn.push((name, bytes.len()));
                        }
                    }
                }
                // The metadata file is a small whole write too.
                if std::fs::read_to_string(dir.join(DISPLAYNAME)).is_ok_and(|text| text.is_empty())
                {
                    torn.push((DISPLAYNAME.to_owned(), 0));
                }
            }
            (reads, torn)
        })
    };
    for round in 1..=3_u32 {
        for n in 0..20 {
            rig.nextcloud
                .put_item("contacts", &format!("big{n}.vcf"), &big(round, n));
        }
        let marker = format!("round-{round}");
        eventually_progressing(
            "the round is mirrored",
            || {
                (0..20)
                    .filter(|n| {
                        read(&contacts.join(format!("big{n}.vcf")))
                            .is_some_and(|t| t.contains(&marker))
                    })
                    .count()
            },
            20,
        )
        .await;
    }
    stop.store(true, Ordering::Relaxed);
    let (reads, torn) = reader.await.expect("reader");
    assert!(
        reads > 200,
        "the reader really ran alongside the writes: {reads}"
    );
    assert!(torn.is_empty(), "torn reads: {torn:?}");
    let leftovers: Vec<_> = std::fs::read_dir(&contacts)
        .expect("dir")
        .map(|e| e.expect("e").file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_the_account_in_syncd_removes_the_mirror_and_it_does_not_come_back() {
    let rig = rig(true).await;
    eventually(&rig, "mirrored", || {
        items_in(&rig.collection("contacts-contacts"), PimKind::Contacts).len() == 2
    })
    .await;
    assert!(rig.account_dir().exists());
    assert_eq!(names(&rig).len(), 1);

    rig.own
        .remove(&AccountId::parse(ACCOUNT).expect("id"))
        .await
        .expect("removed");
    eventually(&rig, "the account's mirror is gone", || {
        !rig.account_dir().exists()
    })
    .await;
    assert!(names(&rig).is_empty(), "{:?}", names(&rig));
    // The supervisor sees no grant any more and starts nothing again.
    rig.after_two_looks().await;
    assert!(!rig.account_dir().exists());
    assert!(names(&rig).is_empty());
    let journals = rig
        .bus
        .scratch()
        .join("syncd/state/porter/sync")
        .join(SEGMENT);
    assert!(!journals.exists(), "its journals went with it");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_grant_stops_and_removes_the_mirror_but_an_unanswering_grant_list_changes_nothing()
 {
    let rig = rig(true).await;
    let contacts = rig.collection("contacts-contacts");
    eventually(&rig, "mirrored", || {
        items_in(&contacts, PimKind::Contacts).len() == 2
    })
    .await;

    // The grants cannot be read: nothing is stopped or removed, and mirroring goes on.
    rig.accountd_up.store(false, Ordering::Relaxed);
    rig.after_two_looks().await;
    assert_eq!(names(&rig).len(), 1);
    rig.nextcloud
        .put_item("contacts", "c3.vcf", &card("c3", "Alan Turing"));
    eventually(&rig, "a running mirror still follows the server", || {
        items_in(&contacts, PimKind::Contacts).len() == 3
    })
    .await;

    // The person withdraws the Contacts grant: that mirror goes.
    rig.accountd_up.store(true, Ordering::Relaxed);
    rig.service
        .revoke(&syncd_app(), &rig.grants[0])
        .await
        .expect("revoke");
    eventually(&rig, "the address book is gone", || !contacts.exists()).await;
    assert!(names(&rig).is_empty(), "{:?}", names(&rig));
}

#[tokio::test(flavor = "multi_thread")]
async fn with_no_grant_nothing_is_mirrored_and_the_server_is_not_even_asked() {
    let rig = rig(false).await;
    rig.after_two_looks().await;
    assert!(!rig.account_dir().exists());
    assert!(names(&rig).is_empty());
    assert_eq!(rig.requests("REPORT") + rig.requests("PROPFIND"), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn syncd_leaves_the_calendars_of_an_account_it_removes_to_calendard() {
    let rig = rig(true).await;
    let contacts = rig.collection("contacts-contacts");
    eventually(&rig, "mirrored", || {
        items_in(&contacts, PimKind::Contacts).len() == 2
    })
    .await;
    // A calendar of the same account that calendard keeps: its directory and its journal.
    let calendar = rig.collection("personal");
    std::fs::create_dir_all(&calendar).expect("calendar dir");
    std::fs::write(
        calendar.join("ev1.ics"),
        "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n",
    )
    .expect("event");
    let journals = rig
        .bus
        .scratch()
        .join("syncd/state/porter/sync")
        .join(SEGMENT);
    let calendard_journal = journals.join("pim_cal_personal.sqlite");
    std::fs::write(&calendard_journal, "journal").expect("journal");

    rig.own
        .remove(&AccountId::parse(ACCOUNT).expect("id"))
        .await
        .expect("removed");
    eventually(&rig, "syncd's address book is gone", || !contacts.exists()).await;
    assert!(calendar.join("ev1.ics").exists(), "calendard's calendar");
    assert!(calendard_journal.exists(), "calendard's journal");
}
