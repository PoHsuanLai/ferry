//! What a feed hands a mirror, sans-IO: the source seam ([`PimSource`], [`Feed`], the cursor and
//! the changes a feed reports), [`FeedReplica`] that makes any feed a `ferry_sync::Replica` for the
//! engine, and the pure helpers every source writes with: the iCalendar and vCard text rules
//! ([`text`]), the way ids become path segments and file names ([`wire`]), and what the Google
//! sources share ([`google`]). Nothing here sends a request, opens a file or names a bus; the
//! mirror (`ferry-pim-mirror`) and each daemon's sources do the I/O.

mod adapter;
mod choice;
mod feed;
pub mod google;
pub mod text;
pub mod wire;

pub use adapter::FeedReplica;
pub use choice::{NoSource, endpoint_of, transport_of};
pub use feed::{DiscoverError, Feed, FeedChange, FeedCursor, Found, Page, PimSource};
