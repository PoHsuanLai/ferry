//! The vdir: calendars, address books and task lists as directories of files on disk (design/31
//! §6.2 "PimMirror", vdirsyncer's storage spec, which sill's `calendar.sources = auto` and khal
//! read).
//!
//! The vdir is `$XDG_DATA_HOME/porter/vdir/<account>/<collection>/`, `<account>` being
//! `porter_ids::object_segment(account)`: the segment `AccountRemoved` wipes. A collection holds
//! one file per item, `<uid>.ics` or `<uid>.vcf`, and the files `displayname` and `color`.
//!
//! - [`PimKind`]: which kind of collection a directory holds, and the names that go with it
//!   (extension, directory suffix, dataset slug, the protocol family and data class a grant for
//!   it covers).
//! - the file rules: names, atomic writes, the collection's metadata files, and what a reader may
//!   rely on ([`is_complete`]).

mod kind;
mod rules;
#[cfg(test)]
mod testing;

pub use kind::PimKind;
pub use rules::{
    COLOR, DISPLAYNAME, Meta, file_name, is_complete, is_item_name, items_in, remove_file, uid_of,
    write_atomic, write_meta,
};
