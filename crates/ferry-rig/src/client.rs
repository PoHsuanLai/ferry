//! What `ferry-rig-client` does: one command as an app, over the session bus, answered as JSON
//! lines. Each command emits one or more objects with a `result` or an `event` key; a failure
//! is `{"result":"error", ...}` and the process exits 1.
//!
//! The commands are the app side of the sync scenarios: list the datasets syncd shows this app,
//! read one's status, settle a stored conflict, watch for conflicts. The app's identity is the
//! daemons' to find (see `porter_rig::fixture`).

use clap::Subcommand;
use ferry_bus::SyncProxy;
use porter_dbus::from_vardict;
use serde_json::{Value, json};
use std::time::Duration;
use zbus::export::futures_core::Stream;

fn failure(why: impl std::fmt::Display) -> Value {
    json!({ "result": "error", "error": why.to_string() })
}

/// The commands: the command line of `ferry-rig-client`.
#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    /// Settles a stored sync conflict.
    SyncResolve {
        /// `<account>/<dataset>`.
        dataset: String,
        /// The conflict's number.
        number: i64,
        /// `keep_local` or `keep_remote`.
        how: String,
    },
    /// Prints the conflicts syncd tells.
    WatchConflicts {
        /// Stop after this many.
        #[arg(long)]
        count: Option<usize>,
        /// Stop after this many seconds.
        #[arg(long = "timeout-s", value_parser = seconds)]
        timeout: Option<Duration>,
    },
    /// The datasets syncd shows this app.
    Datasets,
    /// One dataset's status.
    Status {
        /// `<account>/<dataset>`.
        dataset: String,
    },
    /// Only writes the identity fixture for process `pid` (it is kept); not a bus command.
    WriteIdentity {
        /// The process the fixture names.
        #[arg(long)]
        pid: u32,
    },
}

fn seconds(text: &str) -> Result<Duration, String> {
    text.parse::<u64>()
        .map(Duration::from_secs)
        .map_err(|_| format!("`{text}` is not a whole number of seconds"))
}

/// Runs `command` as the app on `connection`, handing every JSON line to `emit`; whether it
/// succeeded.
pub async fn run(
    connection: &zbus::Connection,
    command: Command,
    emit: &mut impl FnMut(Value),
) -> bool {
    match command {
        Command::SyncResolve {
            dataset,
            number,
            how,
        } => sync_resolve(connection, &dataset, number, &how, emit).await,
        Command::WatchConflicts { count, timeout } => {
            watch_conflicts(connection, count, timeout, emit).await
        }
        Command::WriteIdentity { .. } => {
            emit(failure("write-identity is not a bus command"));
            false
        }
        Command::Datasets => match SyncProxy::new(connection).await {
            Ok(sync) => match sync.datasets().await {
                Ok(names) => {
                    emit(json!({ "result": "ok", "datasets": names }));
                    true
                }
                Err(why) => {
                    emit(failure(why));
                    false
                }
            },
            Err(why) => {
                emit(failure(why));
                false
            }
        },
        Command::Status { dataset } => status(connection, &dataset, emit).await,
    }
}

async fn sync_resolve(
    connection: &zbus::Connection,
    dataset: &str,
    number: i64,
    how: &str,
    emit: &mut impl FnMut(Value),
) -> bool {
    let sync = match SyncProxy::new(connection).await {
        Ok(sync) => sync,
        Err(why) => {
            emit(failure(why));
            return false;
        }
    };
    match sync.resolve(dataset, number, how).await {
        Ok(()) => {
            emit(json!({ "result": "resolved", "dataset": dataset, "number": number, "how": how }));
            true
        }
        Err(zbus::Error::MethodError(name, text, _)) => {
            emit(
                json!({ "result": "error", "name": name.as_str(), "error": text.unwrap_or_default() }),
            );
            false
        }
        Err(why) => {
            emit(failure(why));
            false
        }
    }
}

async fn status(
    connection: &zbus::Connection,
    dataset: &str,
    emit: &mut impl FnMut(Value),
) -> bool {
    let answer = async {
        let sync = SyncProxy::new(connection).await?;
        sync.status(dataset).await
    }
    .await;
    match answer {
        Ok(details) => {
            let fields = from_vardict(&details)
                .map(Value::Object)
                .unwrap_or(Value::Null);
            emit(json!({ "result": "ok", "dataset": dataset, "status": fields }));
            true
        }
        Err(why) => {
            emit(failure(why));
            false
        }
    }
}

async fn watch_conflicts(
    connection: &zbus::Connection,
    count: Option<usize>,
    timeout: Option<Duration>,
    emit: &mut impl FnMut(Value),
) -> bool {
    let sync = match SyncProxy::new(connection).await {
        Ok(sync) => sync,
        Err(why) => {
            emit(failure(why));
            return false;
        }
    };
    let mut conflicts = match sync.receive_conflict().await {
        Ok(stream) => stream,
        Err(why) => {
            emit(failure(why));
            return false;
        }
    };
    // Calling joins syncd's roster: it tells only the connections it has heard from.
    let names = match sync.datasets().await {
        Ok(names) => names,
        Err(why) => {
            emit(failure(why));
            return false;
        }
    };
    emit(json!({ "event": "watching", "datasets": names }));
    let deadline = timeout.map(|t| tokio::time::Instant::now() + t);
    let mut seen = 0usize;
    loop {
        if count.is_some_and(|n| seen >= n) {
            return true;
        }
        let next = std::future::poll_fn(|cx| std::pin::Pin::new(&mut conflicts).poll_next(cx));
        let signal = match deadline {
            Some(at) => match tokio::time::timeout_at(at, next).await {
                Ok(signal) => signal,
                Err(_) => {
                    emit(json!({ "result": "timeout", "seen": seen }));
                    return seen > 0;
                }
            },
            None => next.await,
        };
        let Some(signal) = signal else {
            emit(failure("the signal stream ended"));
            return false;
        };
        let Ok(args) = signal.args() else { continue };
        let conflict = from_vardict(args.conflict())
            .map(Value::Object)
            .unwrap_or(Value::Null);
        emit(json!({ "event": "conflict", "dataset": args.dataset(), "conflict": conflict }));
        seen += 1;
    }
}
