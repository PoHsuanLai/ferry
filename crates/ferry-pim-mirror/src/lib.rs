//! `PimMirror`: calendars, address books and task lists mirrored into a local vdir, read-only
//! (design/31 §6.2 "PimMirror", PLAN §2 rows Calendar and Contacts, D9), and what keeps them
//! running. The vdir is `ferry-vdir`'s, what a feed hands a mirror is `ferry-pim-feed`'s; the
//! daemon that runs this (syncd for contacts, callie's calendard for calendars and tasks) chooses
//! the sources.
//!
//! - `mirror`: [`PimMirror`], the [`ferry_engine::dataset::Dataset`] of one collection, pull-only:
//!   the engine never uploads or removes anything for it, so a file edited or deleted locally
//!   never reaches the server.
//! - `discover`: the collections of an account, from the principal and home sets (dav-proto's
//!   walk).
//! - `plan`: collection names as directories and dataset slugs.
//! - `grants`: which granted accounts there are to mirror (a seam over porter-client).
//! - `relay`, `http`, `google`, `dav`: the connections sources use: relays accountd opens, a JSON
//!   GET, a Google API, and the CalDAV/CardDAV source (the one source every daemon has).
//! - `mirrors`, `supervisor`: one account's collections kept running (replicas over the relay,
//!   one engine each) and the supervisor over every granted account, which asks the daemon's
//!   [`Sources`] to read each grant.
//! - `catalog`: the collections running now, for the daemon that serves them.

mod catalog;
mod dav;
mod discover;
pub mod google;
mod grants;
pub mod http;
mod mirror;
mod mirrors;
mod plan;
mod relay;
mod supervisor;
#[cfg(test)]
mod testing;

pub use catalog::{Catalog, Collection};
pub use dav::{DavCursor, DavFeed, DavSource};
pub use discover::discover;
pub use grants::{AccountdUnavailable, ClientGrants, PimGrants, need_of};
pub use mirror::PimMirror;
pub use mirrors::{AccountMirrors, OpenError, RefreshError, Wiring};
pub use plan::{Planned, plan};
pub use relay::{pim_http, pim_replica};
pub use supervisor::{PimConfig, PimSupervisor, Sources};
