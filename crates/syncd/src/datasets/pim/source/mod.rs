//! Where syncd's collections come from: the sources there are and the choice among them.
//!
//! The seam (`PimSource`, `Feed`) is `ferry-pim-feed`'s and the CalDAV/CardDAV source is
//! `ferry-pim-mirror`'s. The source is chosen by the capability's transport ([`choose`]:
//! `caldav` and `carddav` are DAV, `graph` is [`graph`], `google_api` is [`google`], one source
//! per kind), never by provider.
//!
//! **A new source is one module**: a `PimSource` and a `Feed`, one more arm in [`choose`], one
//! more arm in `SyncdSources::refresh`. Nothing else is touched.

mod civil;
pub mod google;
pub mod graph;

use ferry_pim_feed::{NoSource, transport_of};
use ferry_vdir::PimKind;
use porter_capability::PimTransport;
use porter_core::Candidate;

/// Which source an account's grant for a kind is read through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chosen {
    /// CalDAV or CardDAV.
    Dav,
    /// Microsoft Graph calendars.
    GraphCalendar,
    /// Google Calendar.
    GoogleCalendar,
    /// Google contacts (the People API).
    GooglePeople,
    /// Google Tasks.
    GoogleTasks,
}

/// The source for the account's capability: by its transport, never by provider name.
pub fn choose(kind: PimKind, candidate: &Candidate) -> Result<Chosen, NoSource> {
    let transport = transport_of(kind, candidate);
    match (kind, transport) {
        // A task list over CalDAV is a calendar of `VTODO`s: the CalDAV account's calendars
        // already mirror it as one, so a second mirror of the same collection is not made.
        (PimKind::Tasks, PimTransport::CalDav) => Err(NoSource { kind, transport }),
        (_, PimTransport::CalDav | PimTransport::CardDav) => Ok(Chosen::Dav),
        (PimKind::Calendar, PimTransport::Graph) => Ok(Chosen::GraphCalendar),
        (PimKind::Calendar, PimTransport::GoogleApi) => Ok(Chosen::GoogleCalendar),
        (PimKind::Contacts, PimTransport::GoogleApi) => Ok(Chosen::GooglePeople),
        (PimKind::Tasks, PimTransport::GoogleApi) => Ok(Chosen::GoogleTasks),
        (kind, transport) => Err(NoSource { kind, transport }),
    }
}

#[cfg(test)]
mod tests;
