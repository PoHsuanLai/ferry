//! Starting the fakes: the OAuth issuer, with the tokens `porter-rig` plants in it, and the
//! Graph fake that accepts the tokens the issuer minted. Both listen on `127.0.0.1` on an
//! ephemeral port (the OS gives ports from 32768 up, never below 1024), and everything of the rig
//! lives in one scratch directory.

use crate::rigfile::RigFile;
use ferry_fake_graph::{FakeGraph, GraphHandle};
use porter_fake_servers::{FakeIssuer, IssuerHandle, Running};
use porter_rig::planted::{
    ALL, MAIL_USER, OAUTH_ACCESS_TOKEN, OAUTH_CLIENT, OAUTH_REFRESH_TOKEN, OAUTH_SCOPE,
};
use porter_rig::rigfile::{HttpAt, OauthAt};
use std::io;
use std::path::{Path, PathBuf};

/// How the fakes are started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// The scratch directory (made if missing).
    pub dir: PathBuf,
    /// The `expires_in` of the issuer's access tokens, in seconds (none: the issuer's 3600).
    pub token_lifetime_s: Option<u64>,
}

/// The port of `http://127.0.0.1:<port>`.
fn port_of(url: &str) -> u16 {
    url.rsplit(':')
        .next()
        .and_then(|p| p.trim_end_matches('/').parse().ok())
        .unwrap_or(0)
}

/// The fakes that are running. Dropping it stops them both.
#[derive(Debug)]
pub struct Rig {
    issuer: Running<IssuerHandle>,
    graph: Running<GraphHandle>,
}

impl Rig {
    /// Starts the issuer, plants its tokens, and starts the Graph fake over it.
    pub async fn start(options: &Options) -> io::Result<Self> {
        std::fs::create_dir_all(&options.dir)?;
        let issuer = FakeIssuer::start().await?;
        issuer.seed_refresh_as(OAUTH_REFRESH_TOKEN, OAUTH_CLIENT, OAUTH_SCOPE);
        issuer.seed_access_as(OAUTH_ACCESS_TOKEN, OAUTH_CLIENT);
        if let Some(seconds) = options.token_lifetime_s {
            issuer.set_token_lifetime(seconds);
        }
        let graph = FakeGraph::start_issued(&issuer).await?;
        // The account the Microsoft sign-in reads from `GET /v1.0/me` is the rig's one user.
        graph.set_mail(MAIL_USER);
        Ok(Self { issuer, graph })
    }

    /// The issuer's handle.
    pub fn issuer(&self) -> IssuerHandle {
        (*self.issuer).clone()
    }

    /// The Graph fake's handle.
    pub fn graph(&self) -> GraphHandle {
        (*self.graph).clone()
    }

    /// What `rig.json` says of this rig, for the scratch directory `dir` and the control
    /// endpoint at `control`.
    pub fn describe(&self, dir: &Path, control: &str) -> RigFile {
        let url = self.issuer.base_url().to_owned();
        let graph = self.graph.base_url();
        RigFile {
            dir: dir.to_owned(),
            pid: std::process::id(),
            control: control.to_owned(),
            secrets: ALL.iter().map(|s| (*s).to_owned()).collect(),
            oauth: OauthAt {
                port: port_of(&url),
                authorize: format!("{url}/authorize"),
                token: format!("{url}/token"),
                revoke: format!("{url}/revoke"),
                device: format!("{url}/device"),
                url,
                client_id: OAUTH_CLIENT.to_owned(),
                scope: OAUTH_SCOPE.to_owned(),
                refresh_token: OAUTH_REFRESH_TOKEN.to_owned(),
                access_token: OAUTH_ACCESS_TOKEN.to_owned(),
            },
            graph: HttpAt {
                url: graph.to_owned(),
                port: port_of(graph),
                user: None,
                password: None,
            },
        }
    }
}
