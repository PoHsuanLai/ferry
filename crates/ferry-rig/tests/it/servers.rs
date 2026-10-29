//! `ferry-rig-servers` as a process: it starts, writes `rig.json`, answers its control endpoint
//! and the fakes it started, plants the secrets it says, and ends cleanly on SIGTERM sent to its
//! PID.

use crate::common;

use common::{Proc, eventually};
use ferry_rig::rigfile::RigFile;
use porter_fake_servers::browser::split_loopback;
use porter_fake_servers::http::{Request, Scheme, post_form, send};
use porter_rig::control::call;
use porter_rig::planted;
use serde_json::Value;
use std::net::TcpStream;
use test_kit::Scratch;

fn json(text: &str) -> Value {
    serde_json::from_str(text).expect("JSON")
}

async fn graph_get(url: &str, target: &str, bearer: &str) -> (u16, String) {
    let (address, _) = split_loopback(url).expect("address");
    let request =
        Request::new("GET", target).with_header("Authorization", &format!("Bearer {bearer}"));
    let answer = send(&address, Scheme::Http, &request)
        .await
        .expect("request");
    (answer.status, answer.text())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_rig_starts_lists_what_it_planted_serves_its_levers_and_ends_on_sigterm() {
    let dir = Scratch::new("ferry-rig-servers");
    let mut rig = Proc::spawn(
        env!("CARGO_BIN_EXE_ferry-rig-servers"),
        &[
            "--dir",
            dir.to_str().expect("utf-8"),
            "--token-lifetime-s",
            "7",
        ],
        &[],
    );
    let file = eventually("rig.json is written", || RigFile::read(&dir).ok()).await;

    // What it says: its own PID, ports of an ephemeral kind, every planted secret.
    assert_eq!(file.pid, rig.pid());
    assert_eq!(file.secrets.len(), planted::ALL.len());
    for secret in planted::ALL {
        assert!(file.secrets.iter().any(|s| s == secret), "{secret}");
    }
    assert!(file.oauth.port >= 1024 && file.graph.port >= 1024);
    for port in [file.oauth.port, file.graph.port] {
        TcpStream::connect(("127.0.0.1", port)).expect("every fake listens");
    }
    assert_eq!(file.oauth.access_token, planted::OAUTH_ACCESS_TOKEN);
    assert_eq!(file.oauth.refresh_token, planted::OAUTH_REFRESH_TOKEN);

    // The Graph drive takes the planted access token (it asks the issuer) and no other.
    let drive = |bearer: &'static str| {
        let url = file.graph.url.clone();
        async move { graph_get(&url, "/v1.0/me/drive", bearer).await.0 }
    };
    assert_eq!(drive(planted::OAUTH_ACCESS_TOKEN).await, 200);
    assert_eq!(drive("not-a-token").await, 401);

    // `GET /v1.0/me` names the rig's one user to the planted token, and only to it.
    let (status, text) = graph_get(&file.graph.url, "/v1.0/me", planted::OAUTH_ACCESS_TOKEN).await;
    assert_eq!(status, 200);
    let who = json(&text);
    assert_eq!(who["mail"], planted::MAIL_USER);
    assert_eq!(who["userPrincipalName"], planted::MAIL_USER);
    let (status, _) = graph_get(&file.graph.url, "/v1.0/me", "not-a-token").await;
    assert_eq!(status, 401);

    // The issuer's `expires_in` is the one asked for.
    let (issuer, _) = split_loopback(&file.oauth.url).expect("address");
    let refresh = |token: &str| {
        let pairs = [
            ("grant_type", "refresh_token"),
            ("refresh_token", token),
            ("client_id", planted::OAUTH_CLIENT),
        ]
        .map(|(k, v)| (k.to_owned(), v.to_owned()));
        let issuer = issuer.clone();
        async move {
            let pairs: Vec<(&str, &str)> = pairs
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            post_form(&issuer, "/token", &pairs).await.expect("token")
        }
    };
    let renewed = refresh(planted::OAUTH_REFRESH_TOKEN).await;
    assert_eq!(renewed.status, 200);
    assert_eq!(json(&renewed.text())["expires_in"], 7);

    // Levers: a remote edit shows on the drive, a refused refresh and a revoked refresh token
    // are what the issuer answers.
    let put = call(
        &file.control,
        "POST",
        "/graph/put?path=notes/a.txt",
        b"remote",
    )
    .await;
    assert_eq!(put.expect("put").status, 200);
    let got = call(&file.control, "GET", "/graph/file?path=notes/a.txt", b"")
        .await
        .expect("get");
    assert_eq!(got.body, b"remote");
    let files = call(&file.control, "GET", "/graph/files", b"")
        .await
        .expect("files");
    assert_eq!(json(&files.text())["files"][0]["path"], "notes/a.txt");
    let gone = call(&file.control, "POST", "/graph/delete?path=notes/a.txt", b"")
        .await
        .expect("delete");
    assert_eq!(gone.status, 200);
    let missing = call(&file.control, "GET", "/graph/file?path=notes/a.txt", b"")
        .await
        .expect("get");
    assert_eq!(missing.status, 404);

    let refused = call(&file.control, "POST", "/issuer/refuse-refreshes?n=1", b"")
        .await
        .expect("lever");
    assert_eq!(refused.status, 200);
    let once = refresh(planted::OAUTH_REFRESH_TOKEN).await;
    assert_eq!(once.status, 400, "refused once");
    let revoked = call(&file.control, "POST", "/issuer/revoke", b"")
        .await
        .expect("revoke");
    assert_eq!(revoked.status, 200);
    let after = refresh(planted::OAUTH_REFRESH_TOKEN).await;
    assert_eq!(after.status, 400, "a revoked refresh token is not honoured");
    assert_eq!(
        call(&file.control, "POST", "/nope", b"")
            .await
            .expect("404")
            .status,
        404
    );
    let again = call(&file.control, "GET", "/rig", b"").await.expect("rig");
    assert_eq!(json(&again.text())["pid"], rig.pid());

    // SIGTERM to its PID ends it cleanly and takes rig.json with it.
    let status = rig.terminate();
    assert!(status.success(), "{status:?}");
    assert!(!RigFile::path(&dir).exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn post_stop_ends_the_rig_cleanly() {
    let dir = Scratch::new("ferry-rig-servers-stop");
    let mut rig = Proc::spawn(
        env!("CARGO_BIN_EXE_ferry-rig-servers"),
        &["--dir", dir.to_str().expect("utf-8")],
        &[],
    );
    let file = eventually("rig.json is written", || RigFile::read(&dir).ok()).await;
    let stop = call(&file.control, "POST", "/stop", b"")
        .await
        .expect("stop");
    assert_eq!(stop.status, 200);
    let status = tokio::task::spawn_blocking(move || rig.child.wait().expect("ends"))
        .await
        .expect("join");
    assert!(status.success(), "POST /stop ends it cleanly");
}
