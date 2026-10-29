//! Removing an account: syncd forgets everything of it: its running datasets, its journals and
//! anchors (`<state>/porter/sync/<account>`, the drive listing's `storage_drive` among them), its
//! address books (`<data>/porter/vdir/<account>/<book>-contacts`), its photos library
//! (`<data>/porter/photos/<account>`) and its app folder mirror
//! (`<data>/porter/storage/<account>`), and its drive's downloaded files
//! (`<cache>/porter/drives/<account>`).
//!
//! What is callie's calendard is left where it is: its journals (`pim_cal_*`, `pim_task_*`) and
//! its calendar and task-list directories share the account's two directories. calendard stops
//! and deletes them itself when the grants that let it mirror them are gone.
//!
//! The account is accountd's; syncd's copies of it are removed in its Settings rows
//! (`crate::keep::Keep::remove` calls [`wipe`] and then takes back syncd's grants of it).
//! accountd removing the account does not remove syncd's copies: the person removes them in the
//! sync rows.

use crate::paths::Paths;
use ferry_engine::account::AccountDir;
use ferry_engine::hub::Hub;
use ferry_vdir::PimKind;
use std::io::ErrorKind;
use std::path::Path;

/// Whether `name` is a journal file of calendard's (a calendar's or a task list's, with the
/// files SQLite keeps beside it).
fn calendard_journal(name: &str) -> bool {
    [PimKind::Calendar, PimKind::Tasks]
        .iter()
        .any(|kind| name.starts_with(&format!("{}_", kind.slug())))
}

/// Whether `name` is a directory of the vdir that is not an address book: a calendar or a task
/// list, calendard's.
fn calendard_collection(name: &str) -> bool {
    !name.ends_with(PimKind::Contacts.dir_suffix())
}

/// Removes what is in `dir` that `others` does not call another daemon's, and `dir` itself when
/// nothing of the other's is left; whether `dir` was there. A file already gone is not an error.
fn clear(dir: &Path, others: impl Fn(&str) -> bool) -> std::io::Result<bool> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let mut left = false;
    for entry in entries {
        let entry = entry?;
        if entry.file_name().to_str().is_some_and(&others) {
            left = true;
            continue;
        }
        let path = entry.path();
        let removed = match entry.file_type()?.is_dir() {
            true => std::fs::remove_dir_all(&path),
            false => std::fs::remove_file(&path),
        };
        match removed {
            Err(e) if e.kind() != ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    if !left {
        match std::fs::remove_dir(dir) {
            Err(e) if e.kind() != ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(true)
}

/// Stops the account's datasets, waits until none is in a cycle, and deletes its journals and
/// mirrors (calendard's excepted); how many of its directories existed. A directory already gone
/// is not an error.
pub async fn wipe(paths: &Paths, hub: &Hub, account: &AccountDir) -> std::io::Result<usize> {
    hub.stop_account(account).await;
    let [journals, mirrors] = paths.account_dirs(account);
    let others = [
        paths.photos_dir(account),
        paths.storage_dir(account),
        paths.drive_cache_dir(account),
    ];
    // Mirrors and libraries hold many files: removing them blocks, so it runs off the async
    // threads (the daemon's other accounts keep being served meanwhile).
    tokio::task::spawn_blocking(move || {
        let mut existed = 0;
        existed += usize::from(clear(&journals, calendard_journal)?);
        existed += usize::from(clear(&mirrors, calendard_collection)?);
        for dir in others {
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => existed += 1,
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(existed)
    })
    .await
    .map_err(std::io::Error::other)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::scratch;
    use std::collections::BTreeSet;

    fn paths(root: &Path) -> Paths {
        Paths {
            journals: root.join("state/porter/sync"),
            mirrors: root.join("data/porter/vdir"),
            callers_system: root.join("none"),
            callers_user: root.join("none"),
            cloud: root.join("Cloud"),
            drive_cache: root.join("cache/porter/drives"),
            settings: root.join("none"),
        }
    }

    fn names(dir: &Path) -> BTreeSet<String> {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn a_wipe_removes_that_accounts_journals_and_address_books_and_nothing_else() {
        let _root_guard = scratch("wipe");
        let root = _root_guard.path().to_owned();
        let paths = paths(&root);
        let (gone, kept) = (
            AccountDir::parse("a1").expect("a1"),
            AccountDir::parse("a2").expect("a2"),
        );
        for (account, file) in [(&gone, "files.sqlite"), (&kept, "files.sqlite")] {
            let journal = paths.journal(account, "files");
            std::fs::create_dir_all(journal.parent().expect("dir")).expect("dir");
            std::fs::write(&journal, file).expect("journal");
            let [_, mirror] = paths.account_dirs(account);
            std::fs::create_dir_all(mirror.join("book-contacts")).expect("mirror");
            std::fs::write(mirror.join("book-contacts/x.vcf"), "BEGIN:VCARD").expect("vcf");
        }
        let drive = crate::datasets::drive::journal_path(&paths, &gone);
        crate::datasets::drive::DriveJournal::open(&drive).expect("the drive's journal");
        let hub = Hub::default();
        let name = ferry_engine::hub::DatasetName::parse("a1/files").expect("name");
        let handle = hub.register(name, ferry_engine::hub::Access::default());

        assert_eq!(wipe(&paths, &hub, &gone).await.expect("wipe"), 2);
        let left: BTreeSet<_> = [&paths.journals, &paths.mirrors]
            .iter()
            .flat_map(|dir| names(dir))
            .collect();
        assert_eq!(left, BTreeSet::from(["a2".to_owned()]));
        assert!(!handle.is_registered(), "its engines are stopped");
        assert!(!drive.exists(), "the drive listing goes with the rest");
        assert_eq!(
            wipe(&paths, &hub, &gone).await.expect("again"),
            0,
            "idempotent"
        );
    }

    #[tokio::test]
    async fn a_wipe_leaves_the_calendars_and_task_lists_of_the_account_to_calendard() {
        let _root_guard = scratch("wipe-calendard");
        let root = _root_guard.path().to_owned();
        let paths = paths(&root);
        let account = AccountDir::parse("a1").expect("a1");
        for dataset in [
            "files",
            "pim_card_book_contacts",
            "pim_cal_personal",
            "pim_task_home_tasks",
        ] {
            let journal = paths.journal(&account, dataset);
            std::fs::create_dir_all(journal.parent().expect("dir")).expect("dir");
            std::fs::write(&journal, "journal").expect("journal");
        }
        // The files SQLite keeps beside a journal are the journal's.
        std::fs::write(
            paths
                .journal(&account, "pim_cal_personal")
                .with_extension("sqlite-wal"),
            "wal",
        )
        .expect("wal");
        let [_, mirror] = paths.account_dirs(&account);
        for dir in ["book-contacts", "personal", "home-tasks"] {
            std::fs::create_dir_all(mirror.join(dir)).expect("mirror");
            std::fs::write(mirror.join(dir).join("x"), "item").expect("item");
        }
        let hub = Hub::default();

        assert_eq!(wipe(&paths, &hub, &account).await.expect("wipe"), 2);
        let [journals, mirrors] = paths.account_dirs(&account);
        assert_eq!(
            names(&journals),
            BTreeSet::from([
                "pim_cal_personal.sqlite".to_owned(),
                "pim_cal_personal.sqlite-wal".to_owned(),
                "pim_task_home_tasks.sqlite".to_owned(),
            ])
        );
        assert_eq!(
            names(&mirrors),
            BTreeSet::from(["home-tasks".to_owned(), "personal".to_owned()])
        );
        assert_eq!(
            names(&mirrors.join("personal")),
            BTreeSet::from(["x".to_owned()])
        );
    }
}
