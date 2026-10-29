//! The datasets syncd carries, each a [`ferry_engine::dataset::Dataset`] over some local store.
//!
//! - `contacts`: address books mirrored from a CardDAV server or Google into a local vdir
//!   (W6e), read-only. Calendars and task lists are callie's, not syncd's.
//! - `storage`: the app folder of a Graph account mirrored two ways, and the supervisor that
//!   also runs `photos` for it.
//! - `photos`: originals by content and an HLC-stamped metadata manifest (W6f), off by default.
//! - `drive`: a Storage account's whole drive, listed only (no download), for Files.

pub mod contacts;
pub mod drive;
pub mod photos;
pub mod storage;
