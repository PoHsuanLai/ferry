//! The Google sources: Calendar, People (contacts) and Tasks, each a [`PimSource`] and a
//! [`Feed`] behind the seam, all through the relay accountd opens
//! (`Tokens.OpenAuthenticated`) to the capability's endpoint, so syncd never holds the Google
//! token. One module per API, each with a pure converter to the vdir's formats:
//!
//! | API | collections | changes | converter |
//! |---|---|---|---|
//! | Calendar v3 | `users/me/calendarList` | `events.list` with `syncToken`; `410` is expired | [`event`]: JSON to `VEVENT` |
//! | People v1 | one, "Contacts" | `people.connections.list` with `requestSyncToken`; `EXPIRED_SYNC_TOKEN` | [`person`]: JSON to vCard 3.0 |
//! | Tasks v1 | `users/@me/lists` | `tasks.list` with `updatedMin` and `showDeleted` | [`task`]: JSON to `VTODO` |
//!
//!
//! The API client, the pure helpers and the Google cursor are `ferry-pim-mirror`'s and
//! `ferry-pim-feed`'s; the unverified assumptions about Google are listed in the former's
//! `google` module.

mod calendar;
mod cursor;
pub mod event;
mod people;
pub mod person;
pub mod task;
mod tasks;
mod time;

pub use calendar::{GoogleCalendarFeed, GoogleCalendarSource};
pub use cursor::TaskCursor;
pub use people::{GooglePeopleFeed, GooglePeopleSource};
pub use tasks::{GoogleTasksFeed, GoogleTasksSource};
