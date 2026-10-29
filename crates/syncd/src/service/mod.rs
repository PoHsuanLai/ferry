//! `org.quire.Sync1` at `/org/quire/Sync1`, served over the hub (design/31 §4.4). The object is
//! `ferry-sync-service`'s (callie's calendard serves it too); what follows is what it does.
//!
//! - The caller is the connection's, found by porter-dbus's `Callers` (cgroup, the one table both
//!   daemons share); a sender nothing names is `AccessDenied` and a caller never says who it is.
//! - A dataset is named `<account>/<dataset>` (`Hub`). A caller sees the datasets it owns;
//!   Settings and the porter daemons see all. A name that is unknown, or not the caller's, answers
//!   the refusal `NoFittingAccount` (`org.quire.Accounts1.Error.NoFittingAccount`): nothing is
//!   there for this caller, and no dataset (the state before W6e/W6f) is the same answer, never a
//!   panic. A malformed name is `InvalidArgs`.
//! - `Status` is `anchor_age` (seconds, once there is an anchor), `pending`, `conflicts`,
//!   `paused`, `syncing` (a cycle is running now) and, where the replica reports one, `quota`
//!   (`STATUS_KEY_QUOTA`: `used` and `total`, both `t`).
//! - `Resolve(dataset, conflict, how)` settles a stored conflict (`how` is `keep_local` or
//!   `keep_remote`; the `Conflict` signal's `number` is the conflict). Only the dataset's owning
//!   app may call it (Settings and the porter daemons see a dataset but do not own it: `Denied`);
//!   a dataset the caller cannot see answers `NoFittingAccount` as for the other methods, a
//!   conflict unknown or already settled `org.quire.Sync1.Error.NoSuchConflict`, any other `how`
//!   `InvalidArgs`. The dataset's driver settles it between cycles and runs the next soon.
//! - `ConfirmDiscard(dataset)` lets one discard through for a dataset held with
//!   `needs_confirmation` in `Status`; whoever may pause the dataset may call it (the owning
//!   app, Settings, the porter daemons); a dataset the caller cannot see answers
//!   `NoFittingAccount`, one not held `org.quire.Sync1.Error.NothingHeld`. The driver takes the
//!   request between cycles and runs the next at once. `NeedsConfirmation(dataset, held)` tells
//!   when a hold starts (the same `{discard, held}` as `Status`) and when it ends (empty).
//!   syncd writes no audit events (it has none for pause and resume either), so this is not
//!   audited.
//! - `SyncNow(dataset)` ("Sync now") has a dataset sync at once instead of at its next scheduled
//!   look: `Hub::sync_now` wakes its driver, which runs a cycle right away, or right after the
//!   one running. Whoever may pause the dataset may call it; a dataset the caller cannot see
//!   answers `NoFittingAccount`, a paused one `org.quire.Sync1.Error.Paused`. The reply comes
//!   when the request is taken, not when the cycle ends.
//! - The shell (`SheetHost`) is told `NeedsConfirmation` for every dataset (it carries `account`,
//!   the account's object path, too) but sees nothing else of a dataset it does not own: no
//!   `Progress`, no `Conflict`, and `Status`, `Pause`, `Resume`, `Resolve` and `ConfirmDiscard`
//!   answer `NoFittingAccount`. `Watch()` joins any caller to the connections that are told,
//!   without asking for data, and sends it the holds that are already on.
//! - `Progress`, `Conflict` and `NeedsConfirmation` go to the connections that called and may see the dataset, never
//!   broadcast; a connection that leaves the bus is forgotten.

mod picker;
mod storage;

use ferry_bus::SYNC_BUS;
use ferry_engine::hub::Hub;
use porter_callers::Callers;
use zbus::Connection;

/// Serves `org.quire.Sync1` on `connection` over `hub`, with `callers` saying who calls, and
/// claims `org.quire.Sync1` once the connection takes calls ([`porter_dbus::serve_ready`]). Any
/// other object on `connection` (the drives, the Photos picker) is registered before this is
/// called.
pub async fn serve<C: Callers>(connection: &Connection, hub: Hub, callers: C) -> zbus::Result<()> {
    ferry_sync_service::register(connection, hub, callers).await?;
    // Every object is registered; the name is the promise that calls are taken: claim it only
    // once they are.
    porter_dbus::serve_ready(connection).await?;
    connection.request_name(SYNC_BUS).await?;
    Ok(())
}

pub use ferry_sync_service::RefusedError;
#[cfg(feature = "photos-picker")]
pub use picker::serve_picker;
pub use picker::{PickerDesk, Pickers};
pub use storage::{FilesApps, ITEMS_EVERY, absolute, drive_details, item_details, serve_storage};
