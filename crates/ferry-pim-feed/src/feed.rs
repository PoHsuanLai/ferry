//! The source seam: where a collection's items come from.
//!
//! Everything downstream of a source is shared: the vdir writer (`PimMirror`), the journal, the
//! `PullOnly` rule, the rescan and the supervisor. A source has two jobs and two traits:
//!
//! - [`PimSource`], per account and kind: the collections there are (id, display name, colour,
//!   as a [`Found`]) and a [`Feed`] for each.
//! - [`Feed`], per collection: the changes since a cursor, as upserts that carry the item's
//!   bytes (`.ics` for a VEVENT or a VTODO, `.vcf` for a vCard) and deletes. The cursor type is
//!   the source's own ([`FeedCursor`]: a sync token, a delta link, a page token); the journal
//!   stores it as the opaque anchor text.
//!
//! [`FeedReplica`](crate::FeedReplica) makes any feed a ferry-sync `Replica` for the engine. The
//! source is chosen by the capability's transport (`caldav` and `carddav` are DAV, `graph` is
//! Graph, `google_api` is Google), never by provider.
//!
//! **A new source is one module** in the daemon that runs it: a `PimSource` and a `Feed`, one
//! more arm in that daemon's `Sources`.

use ferry_sync::{More, RemoteItem, ReplicaError, Tombstone};
use ferry_vdir::PimKind;
use porter_ids::WebUrl;
use std::future::Future;

/// One calendar or address book on the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// Its URL, with the trailing slash of a collection.
    pub url: WebUrl,
    /// The last segment of its path, decoded: what it is called in the URL (`personal`).
    pub segment: String,
    /// The name the person gave it.
    pub displayname: Option<String>,
    /// Its colour as the server wrote it.
    pub color: Option<String>,
}

/// Why the collections could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DiscoverError {
    /// The server could not be reached.
    #[error("the server could not be reached")]
    Unreachable,
    /// The server refused the account's credential.
    #[error("the server refused the account")]
    Unauthorized,
    /// The server answered with this status.
    #[error("the server answered {0}")]
    Status(u16),
    /// The answer was not a multistatus, or an href was not a URL.
    #[error("the server's answer could not be read")]
    Unreadable,
}

/// A position in a collection's change feed, in the source's own type.
pub trait FeedCursor: Sized + Send + Sync {
    /// The text the journal keeps as the anchor.
    fn encode(&self) -> String;
    /// The cursor the text stands for; `None` when it is not one of this source's (the feed is
    /// then read again from the start).
    fn decode(text: &str) -> Option<Self>;
}

/// One change of a collection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedChange {
    /// Created or changed. `content` is the item's bytes when the source already holds them
    /// (Graph's delta carries whole events); `None` when they are fetched afterwards (CalDAV's
    /// sync-collection reports names and etags only).
    Upsert {
        /// The item as the journal records it: id, version, name, size, hash.
        item: RemoteItem,
        /// Its iCalendar or vCard bytes, when known.
        content: Option<Vec<u8>>,
    },
    /// Deleted.
    Delete(Tombstone),
}

/// One page of a feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<C> {
    /// The changes, oldest first.
    pub changes: Vec<FeedChange>,
    /// Where the next read starts; stored once the page is applied.
    pub next: C,
    /// Whether more pages follow.
    pub more: More,
}

/// The changes of one collection.
pub trait Feed: Send + Sync + 'static {
    /// The source's cursor type.
    type Cursor: FeedCursor;

    /// Changes after `from`; `None` is the start: everything there is, without deletes. A cursor
    /// the source no longer honours is `ReplicaError::AnchorExpired`.
    fn changes(
        &self,
        from: Option<Self::Cursor>,
    ) -> impl Future<Output = Result<Page<Self::Cursor>, ReplicaError>> + Send;

    /// An item's bytes, for an upsert that came without them.
    fn fetch(
        &self,
        id: &ferry_sync::RemoteId,
    ) -> impl Future<Output = Result<Vec<u8>, ReplicaError>> + Send;

    /// What the backend offers; the default is a read-only feed whose items the engine compares
    /// by SHA-256.
    fn features(&self) -> porter_capability::StorageCap {
        use porter_capability::{Access, Delta, HashKind, StorageCap, StorageScope};
        StorageCap {
            access: Access::Read,
            delta: Delta::Poll,
            quota: false,
            scope: StorageScope::Full,
            hashes: HashKind::Sha256,
            ranges: false,
            chunked_upload: false,
        }
    }

    /// The quota, when `features` says it is reported.
    fn quota(&self) -> impl Future<Output = Result<ferry_sync::Quota, ReplicaError>> + Send {
        async {
            Ok(ferry_sync::Quota {
                used: porter_ids::Bytes(0),
                total: None,
            })
        }
    }
}

/// The collections of an account's kind and the feed of each.
pub trait PimSource: Send + Sync + 'static {
    /// Its feeds.
    type Feed: Feed;

    /// The collections there are now: a name for the directory (`Found::segment`, stable for the
    /// collection), the URL that identifies it, its display name and colour.
    fn collections(
        &self,
        kind: PimKind,
    ) -> impl Future<Output = Result<Vec<Found>, DiscoverError>> + Send;

    /// The feed of one of those collections.
    fn feed(&self, found: &Found) -> Self::Feed;
}
