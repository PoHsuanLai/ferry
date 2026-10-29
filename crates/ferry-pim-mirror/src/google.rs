//! One Google API at one endpoint, reached through the relay accountd opens
//! (`Tokens.OpenAuthenticated`), so the daemon never holds the Google token. Calendar, People
//! and Tasks each have an [`Api`] and read a paged list with [`list_all`]; the sources built on
//! them are the daemons'.
//!
//! **Unverified against Google** (no network in this build; the paths, parameters and error
//! shapes are from Google's published reference as remembered, and the fake serves exactly
//! these; check them when the owner's client first runs): the endpoint paths and query
//! parameters the sources use; that `events.list` with a `syncToken` returns cancelled stubs
//! whatever `showDeleted` says and rejects other filters; that an invalid sync token is `410`
//! with reason `fullSyncRequired`; that a People sync token past its life answers `400` with
//! `EXPIRED_SYNC_TOKEN` in the body; that `updatedMin` is inclusive (the feed copes with either)
//! and that a deleted task stays listable with `showDeleted=true` and `deleted: true`; that
//! `pageSize` up to 1000 and `maxResults` up to 100 (Tasks) and 2500 (Events) are honoured.

use crate::http::get;
use crate::relay::pim_http;
use ferry_pim_feed::DiscoverError;
use ferry_pim_feed::google::rate_limited;
use ferry_pim_feed::wire::{encode, unreached};
use ferry_sync::ReplicaError;
use porter_client::{Grants, GrantsLink};
use porter_core::EndpointUrl;
use porter_core::GrantId;
use porter_http::HttpError;
use porter_http::http::Response;
use porter_http_relay::{RelayDial, RelayHttp};
use porter_ids::WebUrl;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::sync::Arc;

/// The pages a listing of collections is followed for before a looping server is given up on.
const MAX_PAGES: usize = 50;

/// One Google API at one endpoint, reached through the relay.
#[derive(Debug)]
pub struct Api<T: GrantsLink> {
    http: RelayHttp<RelayDial<T>>,
    base: String,
}

impl<T: GrantsLink> Api<T> {
    /// The API at `endpoint` (its version path included), as `grant` allows.
    ///
    /// # Errors
    /// `endpoint` is not a web address.
    pub fn new(
        accounts: Arc<Grants<T>>,
        grant: GrantId,
        endpoint: EndpointUrl,
    ) -> Result<Self, DiscoverError> {
        let base = WebUrl::try_from(&endpoint).map_err(|_| DiscoverError::Unreadable)?;
        Ok(Self {
            base: base.as_str().trim_end_matches('/').to_owned(),
            http: pim_http(accounts, grant, endpoint),
        })
    }

    /// `base` + `path` + the query, each value percent-encoded.
    pub fn url(&self, path: &str, query: &[(&str, &str)]) -> String {
        let query: Vec<String> = query
            .iter()
            .map(|(name, value)| format!("{name}={}", encode(value)))
            .collect();
        match query.is_empty() {
            true => format!("{}{path}", self.base),
            false => format!("{}{path}?{}", self.base, query.join("&")),
        }
    }

    /// The API's base URL, no trailing slash.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// A GET of `url`.
    pub async fn get(&self, url: &str) -> Result<Response<Vec<u8>>, HttpError> {
        get(&self.http, url, None).await
    }

    /// A GET whose failure is a replica's: unreachable is transient.
    pub async fn read(&self, url: &str) -> Result<Response<Vec<u8>>, ReplicaError> {
        self.get(url).await.map_err(unreached)
    }
}

/// One page of a Google list: the items and the token of the next page.
#[derive(Debug, Deserialize)]
struct Listing<I> {
    #[serde(default = "Vec::new")]
    items: Vec<I>,
    #[serde(rename = "nextPageToken")]
    next: Option<String>,
}

/// Every item of a paged list (`items` and `nextPageToken`) at `path`.
///
/// # Errors
/// The server could not be reached, refused the account, answered another status, or answered
/// what is not a list.
pub async fn list_all<T: GrantsLink, I: DeserializeOwned>(
    api: &Api<T>,
    path: &str,
    query: &[(&str, &str)],
) -> Result<Vec<I>, DiscoverError> {
    let mut all = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut asked: Vec<(&str, &str)> = query.to_vec();
        if let Some(page) = token.as_deref() {
            asked.push(("pageToken", page));
        }
        let response = api.get(&api.url(path, &asked)).await.map_err(|e| match e {
            HttpError::Unreachable | HttpError::TimedOut => DiscoverError::Unreachable,
            _ => DiscoverError::Unreadable,
        })?;
        match response.status().as_u16() {
            200 => {}
            403 if rate_limited(&response) => return Err(DiscoverError::Status(429)),
            401 | 403 => return Err(DiscoverError::Unauthorized),
            other => return Err(DiscoverError::Status(other)),
        }
        let page: Listing<I> =
            serde_json::from_slice(response.body()).map_err(|_| DiscoverError::Unreadable)?;
        all.extend(page.items);
        match page.next.filter(|next| !next.is_empty()) {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    Ok(all)
}
