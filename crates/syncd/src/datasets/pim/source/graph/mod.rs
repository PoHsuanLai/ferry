//! The Microsoft Graph calendar source: `GET /me/calendars` for the collections, and per
//! calendar `GET /me/calendars/{id}/events/delta` for the changes, with `@odata.deltaLink` as the
//! cursor and `@removed` items as deletes. Every request goes through the relay accountd opens
//! (`Tokens.OpenAuthenticated`), exactly as the Graph storage mirror does, so syncd never holds
//! the Microsoft token. Events are converted to iCalendar by [`convert`] (pure) and arrive with
//! their bytes: the engine's fetch costs no request.
//!
//! **Unverified against Microsoft's own documentation** (not vendored): that `events/delta` on a
//! single calendar is served as used here (Graph documents `calendarView/delta`, which expands
//! occurrences inside a window and so cannot carry a series master with its `RRULE`), that its
//! deleted items are `{"id", "@removed": {"reason"}}`, and that `Prefer:
//! outlook.body-content-type="text"` applies to it. The fake (`FakeGraph`) serves exactly this.
//! The calendars' list and the `Prefer: odata.maxpagesize` page size are the same as the rest of
//! porter's Graph code uses.

pub mod convert;
mod json;
mod recur;
mod zones;

use ferry_engine::dataset::fingerprint;
use ferry_pim_feed::wire::{decode, encode, name_of, retry_after, unreached};
use ferry_pim_feed::{DiscoverError, Feed, FeedChange, FeedCursor, Found, Page, PimSource};
use ferry_pim_mirror::http::get;
use ferry_pim_mirror::pim_http;
use ferry_sync::{Clock, DELETED};
use ferry_sync::{
    ItemKind, More, RemoteId, RemoteItem, RemoteVersion, ReplicaError, RetryAfter, Tombstone,
};
use ferry_vdir::PimKind;
use json::{Calendar, CalendarPage, Event, EventPage};
use porter_client::{Grants, GrantsLink};
use porter_core::EndpointUrl;
use porter_core::GrantId;
use porter_http::HttpError;
use porter_http::http::Response;
use porter_http_relay::{RelayDial, RelayHttp};
use porter_ids::{Bytes, UnixSeconds, WebUrl};
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// How many events a delta page asks for.
const PAGE: usize = 100;

/// A Graph delta link: the whole URL, which carries the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeltaLink(pub String);

impl FeedCursor for DeltaLink {
    fn encode(&self) -> String {
        self.0.clone()
    }

    fn decode(text: &str) -> Option<Self> {
        text.starts_with("http").then(|| Self(text.to_owned()))
    }
}

/// The calendars of one Microsoft account.
#[derive(Debug)]
pub struct GraphCalendarSource<T: GrantsLink> {
    http: Arc<RelayHttp<RelayDial<T>>>,
    base: WebUrl,
    clock: Clock,
}

impl<T: GrantsLink> GraphCalendarSource<T> {
    /// The source at `endpoint` (`https://graph.microsoft.com`), as `grant` allows.
    pub fn new(
        accounts: Arc<Grants<T>>,
        grant: GrantId,
        endpoint: EndpointUrl,
        clock: Clock,
    ) -> Result<Self, DiscoverError> {
        let base = WebUrl::try_from(&endpoint).map_err(|_| DiscoverError::Unreadable)?;
        Ok(Self {
            http: Arc::new(pim_http(accounts, grant, endpoint)),
            base,
            clock,
        })
    }
}

/// One calendar's feed.
#[derive(Debug)]
pub struct GraphCalendarFeed<T: GrantsLink> {
    http: Arc<RelayHttp<RelayDial<T>>>,
    base: String,
    calendar: String,
    clock: Clock,
}

fn origin(base: &WebUrl) -> String {
    base.as_str().trim_end_matches('/').to_owned()
}

/// The directory name of a calendar: stable (a rename must not move it) and short (Graph's ids
/// are long).
fn segment_of(id: &str) -> String {
    format!("graph-{}", hex::encode(&Sha256::digest(id.as_bytes())[..6]))
}

/// A calendar's colour: its `hexColor` when it has one, else the nearest of Outlook's named
/// colours (approximate hues); `auto` and unknown names have none.
fn color_of(calendar: &Calendar) -> Option<String> {
    if let Some(hex) = calendar.hex_color.as_deref().filter(|h| !h.is_empty()) {
        return Some(hex.to_owned());
    }
    let named = match calendar.color.as_deref()? {
        "lightBlue" => "#4f9bd9",
        "lightGreen" => "#6cbf4b",
        "lightOrange" => "#f2994a",
        "lightGray" => "#9aa0a6",
        "lightYellow" => "#f2c94c",
        "lightTeal" => "#2bb5a8",
        "lightPink" => "#eb7fb5",
        "lightBrown" => "#a47551",
        "lightRed" => "#e0594f",
        _ => return None,
    };
    Some(named.to_owned())
}

fn read_error(response: &Response<Vec<u8>>, now: UnixSeconds) -> ReplicaError {
    match response.status().as_u16() {
        401 | 403 => ReplicaError::Unauthorized,
        404 => ReplicaError::Gone,
        _ => ReplicaError::Transient(retry_after(response, now)),
    }
}

impl<T: GrantsLink + 'static> PimSource for GraphCalendarSource<T> {
    type Feed = GraphCalendarFeed<T>;

    async fn collections(&self, kind: PimKind) -> Result<Vec<Found>, DiscoverError> {
        debug_assert_eq!(
            kind,
            PimKind::Calendar,
            "only calendars are read over Graph here"
        );
        let base = origin(&self.base);
        let mut url = format!("{base}/v1.0/me/calendars");
        let mut found = Vec::new();
        // A user has tens of calendars; a bound keeps a looping server from holding the loop.
        for _ in 0..50 {
            let response = get(&*self.http, &url, None).await.map_err(|e| match e {
                HttpError::Unreachable | HttpError::TimedOut => DiscoverError::Unreachable,
                _ => DiscoverError::Unreadable,
            })?;
            match response.status().as_u16() {
                200 => {}
                401 | 403 => return Err(DiscoverError::Unauthorized),
                other => return Err(DiscoverError::Status(other)),
            }
            let page: CalendarPage =
                serde_json::from_slice(response.body()).map_err(|_| DiscoverError::Unreadable)?;
            for calendar in &page.value {
                let own = format!("{base}/v1.0/me/calendars/{}/", encode(&calendar.id));
                found.push(Found {
                    url: WebUrl::parse(&own).map_err(|_| DiscoverError::Unreadable)?,
                    segment: segment_of(&calendar.id),
                    displayname: calendar.name.clone().filter(|n| !n.is_empty()),
                    color: color_of(calendar),
                });
            }
            match page.next {
                Some(next) if next.starts_with(&base) => url = next,
                _ => return Ok(found),
            }
        }
        Ok(found)
    }

    fn feed(&self, found: &Found) -> GraphCalendarFeed<T> {
        let id = found
            .url
            .path()
            .split('/')
            .rfind(|part| !part.is_empty())
            .map(decode)
            .unwrap_or_default();
        GraphCalendarFeed {
            http: Arc::clone(&self.http),
            base: origin(&self.base),
            calendar: id,
            clock: self.clock.clone(),
        }
    }
}

impl<T: GrantsLink> GraphCalendarFeed<T> {
    fn events_url(&self, tail: &str) -> String {
        format!(
            "{}/v1.0/me/calendars/{}/events{tail}",
            self.base,
            encode(&self.calendar)
        )
    }

    /// An event as the journal records it, with the bytes it converts to; `None` for one that
    /// cannot be written (it is left out and said on standard error).
    fn upsert(&self, event: &Event) -> Option<FeedChange> {
        let ics = match convert::to_ics(event) {
            Ok(ics) => ics.into_bytes(),
            Err(why) => {
                eprintln!("syncd: a calendar event is not mirrored: {why}");
                return None;
            }
        };
        let version = event
            .change_key
            .as_deref()
            .or(event.modified.as_deref())
            .map_or_else(|| fingerprint(&ics).0, str::to_owned);
        Some(FeedChange::Upsert {
            item: RemoteItem {
                id: RemoteId(event.id.clone()),
                version: RemoteVersion(version),
                path: name_of(&event.id),
                size: Bytes(ics.len() as u64),
                hash: Some(fingerprint(&ics)),
                kind: ItemKind::File,
                modified: None,
            },
            content: Some(ics),
        })
    }
}

impl<T: GrantsLink + 'static> Feed for GraphCalendarFeed<T> {
    type Cursor = DeltaLink;

    async fn changes(&self, from: Option<DeltaLink>) -> Result<Page<DeltaLink>, ReplicaError> {
        let resuming = from.is_some();
        let url = match from {
            None => self.events_url("/delta"),
            // A link to another host is not one this source handed out.
            Some(DeltaLink(link)) if link.starts_with(&self.base) => link,
            Some(_) => return Err(ReplicaError::AnchorExpired),
        };
        let prefer = format!("odata.maxpagesize={PAGE}, outlook.body-content-type=\"text\"");
        let response = get(&*self.http, &url, Some(&prefer))
            .await
            .map_err(unreached)?;
        match (response.status().as_u16(), resuming) {
            (200, _) => {}
            // `syncStateNotFound` and its kin: the server dropped what the link stands for.
            (410, true) => return Err(ReplicaError::AnchorExpired),
            _ => return Err(read_error(&response, self.clock.now())),
        }
        let page: EventPage = serde_json::from_slice(response.body())
            .map_err(|_| ReplicaError::Transient(RetryAfter(30)))?;
        let (next, more) = match (page.next, page.delta) {
            (Some(next), _) => (next, More::More),
            (None, Some(delta)) => (delta, More::Done),
            (None, None) => return Err(ReplicaError::Transient(RetryAfter(30))),
        };
        let changes = page
            .value
            .iter()
            .filter_map(|event| match event.removed {
                Some(_) => Some(FeedChange::Delete(Tombstone {
                    id: RemoteId(event.id.clone()),
                    version: RemoteVersion(DELETED.to_owned()),
                    deleted_at: self.clock.now(),
                })),
                None => self.upsert(event),
            })
            .collect();
        Ok(Page {
            changes,
            next: DeltaLink(next),
            more,
        })
    }

    async fn fetch(&self, id: &RemoteId) -> Result<Vec<u8>, ReplicaError> {
        let url = self.events_url(&format!("/{}", encode(&id.0)));
        let prefer = "outlook.body-content-type=\"text\"";
        let response = get(&*self.http, &url, Some(prefer))
            .await
            .map_err(unreached)?;
        if response.status().as_u16() != 200 {
            return Err(read_error(&response, self.clock.now()));
        }
        let event: Event = serde_json::from_slice(response.body())
            .map_err(|_| ReplicaError::Transient(RetryAfter(30)))?;
        convert::to_ics(&event)
            .map(String::into_bytes)
            .map_err(|_| ReplicaError::Gone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_name_is_short_stable_and_safe_whatever_the_id() {
        let id = "AAMkAGI2TG93AAA=/with+odd/chars";
        assert_eq!(segment_of(id), segment_of(id));
        assert_ne!(segment_of(id), segment_of("other"));
        assert!(segment_of(id).len() < 20);
    }

    #[test]
    fn a_colour_is_the_hex_one_else_a_named_one_else_none() {
        let cal = |hex: Option<&str>, color: Option<&str>| Calendar {
            id: "1".into(),
            name: None,
            hex_color: hex.map(str::to_owned),
            color: color.map(str::to_owned),
        };
        assert_eq!(
            color_of(&cal(Some("#0078d4"), Some("lightBlue"))).as_deref(),
            Some("#0078d4")
        );
        assert_eq!(
            color_of(&cal(Some(""), Some("lightGreen"))).as_deref(),
            Some("#6cbf4b")
        );
        assert_eq!(color_of(&cal(None, Some("auto"))), None);
        assert_eq!(color_of(&cal(None, None)), None);
    }
}
