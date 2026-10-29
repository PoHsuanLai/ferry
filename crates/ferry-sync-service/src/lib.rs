//! The dataset control of ferry-engine, on the bus: `org.quire.Sync1` (ferry-bus's interface)
//! served over a `Hub` for whoever is calling, found by porter-callers. A dataset is
//! `<account>/<dataset>`; a caller sees the datasets it owns, Settings and the porter daemons see
//! all, and a name that is unknown or not the caller's is `NoFittingAccount`.
//!
//! - `object`: [`register`], the `Sync1` object and the signal relay (progress, conflict, hold).
//! - `status`: the vardicts it sends (`Status`, `NeedsConfirmation`, `Progress`, `Conflict`).
//! - `errors`: [`RefusedError`], what a method answers instead of a value, which the daemon's
//!   other objects answer with too.
//!
//! A daemon registers it, registers whatever else it serves, then claims its bus name.

mod errors;
mod object;
mod status;

pub use errors::RefusedError;
pub use object::register;
pub use status::{conflict_details, held_details, progress_details, status_details};
