//! `ferry-rig-client` as a process, on a private bus with syncd's `Sync1` served over a hub. The
//! daemon names the client from the identity fixture the client writes under a proc root
//! (`porter_rig::fixture`), the way a scenario's `test-proc-root` daemons do: the client is
//! `org.example.Rig`, a Flatpak app, and nothing else says so.

use crate::common;

use common::bus::PrivateBus;
use ferry_sync::{BaseVersion, Conflict, RemoteId, RemoteSide, RemoteVersion, StoredConflict};
use porter_callers::{CallerTable, ProcCallers};
use porter_core::AppName;
use porter_ids::UnixSeconds;
use serde_json::Value;
use std::collections::BTreeSet;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use syncd::service::{Access, DatasetName, Event, Hub, Nudge, serve};
use test_kit::Scratch;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

const APP: &str = "org.example.Rig";
const DATASET: &str = "a1/notes";

struct Rig {
    bus: PrivateBus,
    hub: Hub,
    settled: Arc<Mutex<Vec<(i64, String)>>>,
    _keep: Vec<zbus::Connection>,
    /// Last, so it goes after the bus and the connections.
    proc_root: Scratch,
}

impl Rig {
    /// `ferry-rig-client --app-id org.example.Rig --proc-root <root> <args>`, on the bus.
    fn client(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ferry-rig-client"));
        command
            .env_clear()
            .env("DBUS_SESSION_BUS_ADDRESS", self.bus.address())
            .args(["--app-id", APP, "--proc-root"])
            .arg(self.proc_root.path())
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        command
    }

    /// Runs the client to its end: its JSON lines and whether it succeeded.
    async fn run(&self, args: &[&str]) -> (Vec<Value>, bool) {
        let output = self.client(args).output().await.expect("the client runs");
        let lines = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|l| serde_json::from_str(l).expect("a JSON line"))
            .collect();
        (lines, output.status.success())
    }
}

async fn rig() -> Rig {
    let bus = PrivateBus::start();
    let proc_root = Scratch::new("client-proc");
    let hub = Hub::default();
    let server = bus.connect().await;
    // The daemon names callers from the fixture the client writes, as a scenario's does.
    let callers = Arc::new(ProcCallers::with_proc_root(
        server.clone(),
        CallerTable::default(),
        proc_root.to_path_buf(),
    ));
    serve(&server, hub.clone(), callers).await.expect("sync1");
    let handle = hub.register(
        DatasetName::parse(DATASET).expect("name"),
        Access {
            owners: BTreeSet::from([AppName::parse(APP).expect("app")]),
        },
    );
    // The dataset's driver, as far as settling goes: it records what the owner asks.
    let settled = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&settled);
    tokio::spawn(async move {
        let mut own = handle;
        loop {
            match own.nudged().await {
                Nudge::Settle(request) => {
                    recorded
                        .lock()
                        .expect("lock")
                        .push((request.number.0, format!("{:?}", request.how.0)));
                    let _ = request.reply.send(Ok(()));
                }
                Nudge::Pause | Nudge::Confirm(_) | Nudge::SyncNow => {}
                Nudge::Dropped => break,
            }
        }
    });
    Rig {
        bus,
        hub,
        settled,
        _keep: vec![server],
        proc_root,
    }
}

fn result_of(lines: &[Value]) -> &str {
    lines
        .last()
        .and_then(|l| l["result"].as_str())
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_client_lists_datasets_watches_conflicts_and_settles_one() {
    let rig = rig().await;
    let (lines, ok) = rig.run(&["datasets"]).await;
    assert!(ok, "{lines:?}");
    assert_eq!(lines[0]["datasets"][0], DATASET);

    // watch-conflicts joins syncd's roster, says so, and prints the next conflict.
    let mut watching = rig
        .client(&["watch-conflicts", "--count", "1", "--timeout-s", "60"])
        .spawn()
        .expect("spawns");
    let stdout = watching.stdout.take().expect("piped");
    let mut lines = BufReader::new(stdout).lines();
    let first = tokio::time::timeout(test_kit::GENEROUS, lines.next_line())
        .await
        .expect("a line in time")
        .expect("read")
        .expect("a line");
    let first: Value = serde_json::from_str(&first).expect("JSON");
    assert_eq!(first["event"], "watching");
    assert_eq!(first["datasets"][0], DATASET);

    let name = DatasetName::parse(DATASET).expect("name");
    let stored = StoredConflict {
        number: Some(7),
        conflict: Conflict {
            item: RemoteId("r1".into()),
            base: BaseVersion::Absent,
            remote: RemoteSide::Exists(RemoteId("r1".into()), RemoteVersion("v2".into())),
        },
        local: ferry_sync::LocalId("a.txt".into()),
        local_hash: None,
        at: UnixSeconds(5),
    };
    // Any handle of the hub may tell: the dataset's own is held by the driver task, so a second
    // registration under another name would not be seen; use the event the hub relays.
    let teller = rig.hub.register(
        DatasetName::parse("a1/other").expect("name"),
        Access {
            owners: BTreeSet::from([AppName::parse(APP).expect("app")]),
        },
    );
    teller.tell(Event::Conflict {
        dataset: name,
        conflict: Box::new(stored),
    });
    let second = tokio::time::timeout(test_kit::GENEROUS, lines.next_line())
        .await
        .expect("a line in time")
        .expect("read")
        .expect("a line");
    let second: Value = serde_json::from_str(&second).expect("JSON");
    assert_eq!(second["event"], "conflict");
    assert_eq!(second["dataset"], DATASET);
    assert_eq!(second["conflict"]["number"], 7);
    let status = watching.wait().await.expect("ends");
    assert!(status.success(), "--count 1 ends it after one");

    let (lines, ok) = rig
        .run(&["sync-resolve", DATASET, "7", "keep_remote"])
        .await;
    assert!(ok, "{lines:?}");
    assert_eq!(result_of(&lines), "resolved");
    assert_eq!(
        rig.settled.lock().expect("lock").as_slice(),
        [(7, "KeepRemote".to_owned())]
    );

    let (lines, ok) = rig.run(&["sync-resolve", DATASET, "7", "keep_both"]).await;
    assert!(!ok);
    assert_eq!(result_of(&lines), "error");
    assert!(
        lines[0]["name"]
            .as_str()
            .is_some_and(|n| n.contains("InvalidArgs")),
        "{lines:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_client_removes_its_identity_when_it_ends() {
    let rig = rig().await;
    let (lines, ok) = rig.run(&["datasets"]).await;
    assert!(ok, "{lines:?}");
    let left: Vec<_> = std::fs::read_dir(rig.proc_root.path())
        .expect("proc")
        .collect();
    assert!(left.is_empty(), "{left:?}");
}
