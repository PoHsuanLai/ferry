//! Which kind of collection a directory of the vdir holds.

use porter_core::{DataClass, Family};

/// Which kind of collection a directory holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PimKind {
    /// Calendars (CalDAV): `.ics` files.
    Calendar,
    /// Address books (CardDAV): `.vcf` files.
    Contacts,
    /// Task lists: `.ics` files holding a `VTODO`, in their own directories.
    Tasks,
}

impl PimKind {
    /// The file extension of an item, with its dot.
    pub fn extension(self) -> &'static str {
        match self {
            PimKind::Calendar | PimKind::Tasks => ".ics",
            PimKind::Contacts => ".vcf",
        }
    }

    /// The `BEGIN` and `END` lines a whole item opens and closes with.
    pub fn envelope(self) -> (&'static str, &'static str) {
        match self {
            PimKind::Calendar | PimKind::Tasks => ("BEGIN:VCALENDAR", "END:VCALENDAR"),
            PimKind::Contacts => ("BEGIN:VCARD", "END:VCARD"),
        }
    }

    /// The protocol family that reaches it over DAV (a task list is a CalDAV collection of
    /// `VTODO`s).
    pub fn family(self) -> Family {
        match self {
            PimKind::Calendar | PimKind::Tasks => Family::CalDav,
            PimKind::Contacts => Family::CardDav,
        }
    }

    /// The data class a grant for it covers.
    pub fn class(self) -> DataClass {
        match self {
            PimKind::Calendar => DataClass::Calendar,
            PimKind::Contacts => DataClass::Contacts,
            PimKind::Tasks => DataClass::Tasks,
        }
    }

    /// What its directories end in: nothing for a calendar, `-contacts` for an address book,
    /// `-tasks` for a task list (so the three never share a directory).
    pub fn dir_suffix(self) -> &'static str {
        match self {
            PimKind::Calendar => "",
            PimKind::Contacts => "-contacts",
            PimKind::Tasks => "-tasks",
        }
    }

    /// The prefix of its datasets' names (`pim_cal_personal`, `pim_card_contacts_contacts`).
    pub fn slug(self) -> &'static str {
        match self {
            PimKind::Calendar => "pim_cal",
            PimKind::Contacts => "pim_card",
            PimKind::Tasks => "pim_task",
        }
    }
}
