//! The Google contacts source (People): a [`PimSource`](ferry_pim_feed::PimSource) and a
//! [`Feed`](ferry_pim_feed::Feed) behind the seam, through the relay accountd opens
//! (`Tokens.OpenAuthenticated`) to the capability's endpoint, so syncd never holds the Google
//! token, with a pure converter to the vdir's format:
//!
//! | API | collections | changes | converter |
//! |---|---|---|---|
//! | People v1 | one, "Contacts" | `people.connections.list` with `requestSyncToken`; `EXPIRED_SYNC_TOKEN` | [`person`]: JSON to vCard 3.0 |
//!
//! The API client, the pure helpers and the Google cursor are `ferry-pim-mirror`'s and
//! `ferry-pim-feed`'s; the unverified assumptions about Google are listed in the former's
//! `google` module. Google Calendar and Google Tasks are callie's.

mod people;
pub mod person;

pub use people::{GooglePeopleFeed, GooglePeopleSource};
