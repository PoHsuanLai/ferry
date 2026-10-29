//! `ferry-rig-servers --dir <scratch> [--token-lifetime-s N]`
//!
//! Starts the OAuth issuer and the Graph fake on 127.0.0.1 (the fake accepts the tokens the
//! issuer minted, the planted access token among them), writes `<scratch>/rig.json`, serves the
//! control endpoint (see `ferry_rig::control`) and waits. SIGTERM, SIGINT or `POST /stop` ends it
//! cleanly and removes `rig.json`.

use clap::Parser;
use ferry_rig::control::{Levers, serve_levers};
use ferry_rig::rigfile::RigFile;
use ferry_rig::servers::{Options, Rig};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::Notify;

/// Fake servers for a jailed scenario (test tooling, never installed).
#[derive(Debug, Parser)]
#[command(name = "ferry-rig-servers", version)]
struct Args {
    /// The scratch directory: rig.json goes here.
    #[arg(long)]
    dir: PathBuf,
    /// The `expires_in` of the issuer's access tokens, in seconds (the issuer's own: 3600). A
    /// short one makes a daemon refresh again inside a scenario.
    #[arg(long, value_name = "SECONDS")]
    token_lifetime_s: Option<u64>,
}

fn fail(why: impl std::fmt::Display) -> ExitCode {
    eprintln!("ferry-rig-servers: {why}");
    ExitCode::FAILURE
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    let options = Options {
        dir: args.dir.clone(),
        token_lifetime_s: args.token_lifetime_s,
    };
    let Ok(mut term) = signal(SignalKind::terminate()) else {
        return fail("cannot listen for SIGTERM");
    };
    let rig = match Rig::start(&options).await {
        Ok(rig) => rig,
        Err(why) => return fail(format!("cannot start the fakes: {why}")),
    };
    let stop = Arc::new(Notify::new());
    let levers = Levers {
        issuer: rig.issuer(),
        graph: rig.graph(),
        rig: Arc::new(Mutex::new(String::new())),
        stop: Arc::clone(&stop),
    };
    let (control, _control) = match serve_levers(levers.clone()).await {
        Ok(served) => served,
        Err(why) => return fail(format!("cannot serve the control endpoint: {why}")),
    };
    let file = rig.describe(&args.dir, &control);
    match serde_json::to_string_pretty(&file) {
        Ok(text) => {
            *levers
                .rig
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = text;
        }
        Err(why) => return fail(why),
    }
    if let Err(why) = file.write(&args.dir) {
        return fail(format!("cannot write rig.json: {why}"));
    }
    eprintln!(
        "ferry-rig-servers: ready, {}",
        RigFile::path(&args.dir).display()
    );
    tokio::select! {
        _ = term.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
        () = stop.notified() => {}
    }
    let _ = std::fs::remove_file(RigFile::path(&args.dir));
    drop(rig);
    ExitCode::SUCCESS
}
