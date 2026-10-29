//! The control endpoint: the scenario's levers on the fakes, a small HTTP server on loopback in
//! the rig's own process. `curl` is enough to drive it. Every answer is JSON (`{"ok":true,..}`)
//! except where a route says it returns bytes; an unknown route is `404`, a missing or bad
//! parameter `400`.
//!
//! | route | does |
//! |---|---|
//! | `GET /rig` | the text of `rig.json` |
//! | `POST /issuer/refuse-refreshes?n=N` | the next N refresh requests get `invalid_grant` |
//! | `POST /issuer/revoke[?token=T]` | revokes T at the issuer (default: the planted refresh token) |
//! | `POST /graph/put?path=P` (body) | a remote edit: creates or replaces file P in the app folder |
//! | `POST /graph/delete?path=P` | a remote delete of P |
//! | `GET /graph/file?path=P` | the bytes of P (`404` when there is none) |
//! | `GET /graph/files` | `{"files":[{"path":..,"size":..}]}` |
//! | `POST /graph/expire-delta` | every delta token handed out is stale (`410`) |
//! | `GET /graph/hits` | `{"hits":[{"method","target","status","bearer"}]}` (never the token) |
//! | `POST /graph/calendars-seed` | fills the Graph fake's calendars with a small set |
//! | `POST /graph/calendar?id=I[&name=N][&color=C]` | adds or renames a calendar |
//! | `POST /graph/event?calendar=C&id=I` (body: Graph event JSON), `POST /graph/event-remove?calendar=C&id=I` | a remote edit or delete of an event |
//! | `POST /stop` | ends the rig as SIGTERM does |

use ferry_fake_graph::GraphHandle;
use porter_fake_servers::http::{Request, Response, post_form, serve};
use porter_fake_servers::net::{Bind, Listener};
use porter_fake_servers::{IssuerHandle, browser::split_loopback};
use porter_rig::planted::OAUTH_REFRESH_TOKEN;
use serde_json::{Value, json};
use std::io;
use std::sync::Arc;
use tokio::sync::Notify;

/// What the levers act on.
#[derive(Debug, Clone)]
pub struct Levers {
    /// The issuer.
    pub issuer: IssuerHandle,
    /// The Graph fake.
    pub graph: GraphHandle,
    /// The text `GET /rig` answers.
    pub rig: Arc<std::sync::Mutex<String>>,
    /// Rung by `POST /stop`.
    pub stop: Arc<Notify>,
}

fn ok(extra: Value) -> Response {
    let mut body = json!({ "ok": true });
    if let (Some(into), Some(from)) = (body.as_object_mut(), extra.as_object()) {
        into.extend(from.clone());
    }
    Response::json(200, &body)
}

fn refused(status: u16, why: &str) -> Response {
    Response::json(status, &json!({ "ok": false, "error": why }))
}

/// Runs an async step from the handler (which is a plain function on a runtime thread).
fn run<T>(work: impl std::future::Future<Output = T>) -> T {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(work))
}

impl Levers {
    /// Answers one request.
    pub fn answer(&self, request: &Request) -> Response {
        let query = |name: &str| request.query_value(name);
        let (issuer, graph) = (&self.issuer, &self.graph);
        match (request.method.as_str(), request.path()) {
            ("GET", "/rig") => {
                let text = self
                    .rig
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                Response::new(200).typed("application/json", text)
            }
            ("POST", "/issuer/refuse-refreshes") => {
                match query("n").and_then(|n| n.parse::<u32>().ok()) {
                    None => refused(400, "n is a number"),
                    Some(n) => {
                        issuer.refuse_refreshes(n);
                        ok(json!({ "n": n }))
                    }
                }
            }
            ("POST", "/issuer/revoke") => {
                let token = query("token").unwrap_or_else(|| OAUTH_REFRESH_TOKEN.to_owned());
                let Ok((address, _)) = split_loopback(issuer.base_url()) else {
                    return refused(500, "the issuer has no loopback address");
                };
                let sent = run(post_form(&address, "/revoke", &[("token", &token)]));
                match sent {
                    Ok(answer) if answer.status == 200 => ok(json!({})),
                    _ => refused(502, "the issuer did not take the revoke"),
                }
            }
            ("POST", "/graph/put") => match query("path") {
                None => refused(400, "path is required"),
                Some(path) => {
                    graph.put_file(&path, &request.body);
                    ok(json!({ "path": path, "size": request.body.len() }))
                }
            },
            ("POST", "/graph/delete") => match query("path") {
                None => refused(400, "path is required"),
                Some(path) => {
                    graph.delete(&path);
                    ok(json!({ "path": path }))
                }
            },
            ("POST", "/graph/calendars-seed") => {
                graph.seed_calendars();
                ok(json!({}))
            }
            ("POST", "/graph/calendar") => match query("id") {
                None => refused(400, "id is required"),
                Some(id) => {
                    let name = query("name").unwrap_or_else(|| id.clone());
                    graph.set_calendar(&id, &name, &query("color").unwrap_or_default());
                    ok(json!({ "id": id }))
                }
            },
            ("POST", "/graph/event") => match (query("calendar"), query("id")) {
                (Some(calendar), Some(id)) => {
                    match serde_json::from_slice::<Value>(&request.body) {
                        Ok(event) => {
                            graph.put_event(&calendar, &id, event);
                            ok(json!({ "calendar": calendar, "id": id }))
                        }
                        Err(_) => refused(400, "the body is not JSON"),
                    }
                }
                _ => refused(400, "calendar and id are required"),
            },
            ("POST", "/graph/event-remove") => match (query("calendar"), query("id")) {
                (Some(calendar), Some(id)) => {
                    graph.remove_event(&calendar, &id);
                    ok(json!({ "calendar": calendar, "id": id }))
                }
                _ => refused(400, "calendar and id are required"),
            },
            ("GET", "/graph/file") => match query("path") {
                None => refused(400, "path is required"),
                Some(path) => match graph.file(&path) {
                    Some(bytes) => Response::new(200).typed("application/octet-stream", bytes),
                    None => refused(404, "no such file"),
                },
            },
            ("GET", "/graph/files") => {
                let files: Vec<Value> = graph
                    .files()
                    .into_iter()
                    .map(|(path, size)| json!({ "path": path, "size": size }))
                    .collect();
                ok(json!({ "files": files }))
            }
            ("POST", "/graph/expire-delta") => {
                graph.expire_delta_tokens();
                ok(json!({}))
            }
            ("GET", "/graph/hits") => {
                let hits: Vec<Value> = graph
                    .hits()
                    .iter()
                    .map(|h| {
                        json!({
                            "method": h.method, "target": h.target, "status": h.status,
                            "bearer": h.authorization.is_some(),
                        })
                    })
                    .collect();
                ok(json!({ "hits": hits }))
            }
            ("POST", "/stop") => {
                self.stop.notify_one();
                ok(json!({}))
            }
            _ => refused(404, "no such lever"),
        }
    }
}

/// Binds the control endpoint on loopback and serves it on a task; its URL.
pub async fn serve_levers(levers: Levers) -> io::Result<(String, tokio::task::JoinHandle<()>)> {
    let listener = Listener::bind(&Bind::Port(0), "control").await?;
    let port = porter_fake_servers::net::port_of(listener.address());
    let task = tokio::spawn(serve(
        listener,
        None,
        Arc::new(move |request: Request| levers.answer(&request)),
    ));
    Ok((format!("http://127.0.0.1:{port}"), task))
}
