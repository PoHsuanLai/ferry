//! What the Google sources share, pure: the cursor of Calendar and People, what an error body
//! says, what a failed read means to a replica, and how a collection's directory is named. The
//! requests are the daemon's.
//!
//! Calendar and People hand out a sync token when a listing is finished and a page token while
//! it is not; a page token only works with the parameters the listing began with, so a cursor in
//! the middle of a listing keeps the sync token it began with. Values are percent-encoded so the
//! text splits on `:`.

use crate::FeedCursor;
use crate::wire::{decode, encode, retry_after};
use ferry_sync::ReplicaError;
use porter_http::http::Response;
use porter_ids::{UnixSeconds, WebUrl};
use sha2::{Digest, Sha256};

/// A position in an `events.list` or `people.connections.list` read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncCursor {
    /// The listing finished and handed out this sync token.
    Synced(String),
    /// The listing is on a later page.
    Paging {
        /// The token of the page to read next.
        page: String,
        /// The sync token the listing began with, when it was incremental.
        sync: Option<String>,
    },
}

impl FeedCursor for SyncCursor {
    fn encode(&self) -> String {
        match self {
            SyncCursor::Synced(token) => format!("sync:{}", encode(token)),
            SyncCursor::Paging { page, sync } => format!(
                "page:{}:{}",
                encode(page),
                sync.as_deref().map(encode).unwrap_or_default()
            ),
        }
    }

    fn decode(text: &str) -> Option<Self> {
        let (tag, rest) = text.split_once(':')?;
        match tag {
            "sync" => (!rest.is_empty()).then_some(SyncCursor::Synced(decode(rest))),
            "page" => {
                let (page, sync) = rest.split_once(':')?;
                (!page.is_empty()).then(|| SyncCursor::Paging {
                    page: decode(page),
                    sync: Some(sync).filter(|s| !s.is_empty()).map(decode),
                })
            }
            _ => None,
        }
    }
}

/// Whether an error body says the caller is going too fast (Google answers some of these `403`).
pub fn rate_limited(response: &Response<Vec<u8>>) -> bool {
    let body = String::from_utf8_lossy(response.body());
    [
        "rateLimitExceeded",
        "userRateLimitExceeded",
        "quotaExceeded",
    ]
    .iter()
    .any(|reason| body.contains(reason))
}

/// What a failed read means to a replica.
pub fn read_error(response: &Response<Vec<u8>>, now: UnixSeconds) -> ReplicaError {
    match response.status().as_u16() {
        403 if rate_limited(response) => ReplicaError::Transient(retry_after(response, now)),
        401 | 403 => ReplicaError::Unauthorized,
        404 => ReplicaError::Gone,
        _ => ReplicaError::Transient(retry_after(response, now)),
    }
}

/// The directory name of a collection: stable under renames and short (Google's calendar ids are
/// addresses and can be long).
pub fn segment_of(id: &str) -> String {
    format!(
        "google-{}",
        hex::encode(&Sha256::digest(id.as_bytes())[..6])
    )
}

/// The id a collection's URL (`.../calendars/<id>/`) was made from.
pub fn id_in(url: &WebUrl) -> String {
    url.path()
        .split('/')
        .rfind(|part| !part.is_empty())
        .map(crate::wire::decode)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: u16, body: &str) -> Response<Vec<u8>> {
        Response::builder()
            .status(status)
            .header("Retry-After", "7")
            .body(body.as_bytes().to_vec())
            .expect("response")
    }

    #[test]
    fn a_failed_read_means_what_google_meant() {
        use ReplicaError as E;
        let limited = r#"{"error":{"errors":[{"reason":"userRateLimitExceeded"}]}}"#;
        let denied = r#"{"error":{"errors":[{"reason":"insufficientPermissions"}]}}"#;
        let cases = [
            (401, "", E::Unauthorized),
            (403, denied, E::Unauthorized),
            (403, limited, E::Transient(ferry_sync::RetryAfter(7))),
            (404, "", E::Gone),
            (429, "", E::Transient(ferry_sync::RetryAfter(7))),
            (503, "", E::Transient(ferry_sync::RetryAfter(7))),
        ];
        for (status, body, want) in cases {
            assert_eq!(
                read_error(&response(status, body), UnixSeconds(0)),
                want,
                "{status}"
            );
        }
    }

    #[test]
    fn a_collection_directory_is_short_stable_and_made_of_safe_characters() {
        let id = "en.usa#holiday@group.v.calendar.google.com";
        assert_eq!(segment_of(id), segment_of(id));
        assert_ne!(segment_of(id), segment_of("ada@gmail.com"));
        assert!(segment_of(id).len() < 20);
        assert!(
            segment_of(id)
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
        );
    }

    #[test]
    fn a_collection_url_gives_back_the_id_it_was_made_from() {
        for id in [
            "ada@gmail.com",
            "en.usa#holiday@group.v.calendar.google.com",
            "MTIzNDU2",
        ] {
            let url = WebUrl::parse(&format!("https://x.test/v3/calendars/{}/", encode(id)))
                .expect("url");
            assert_eq!(id_in(&url), id);
        }
    }

    #[test]
    fn a_sync_cursor_round_trips_whatever_the_tokens_hold() {
        for cursor in [
            SyncCursor::Synced("CJDCkdHaxfICEJDC=".into()),
            SyncCursor::Synced("with:colon and space/slash%".into()),
            SyncCursor::Paging {
                page: "EAE".into(),
                sync: None,
            },
            SyncCursor::Paging {
                page: "a:b".into(),
                sync: Some("s:t".into()),
            },
        ] {
            let text = cursor.encode();
            assert_eq!(SyncCursor::decode(&text), Some(cursor), "{text}");
        }
    }

    #[test]
    fn text_that_is_not_a_sync_cursor_is_not_one() {
        for text in [
            "",
            "sync",
            "sync:",
            "page:",
            "page:x",
            "token",
            "https://graph.test/delta?x=1",
            "since:2026",
        ] {
            assert_eq!(SyncCursor::decode(text), None, "{text:?}");
        }
    }
}
