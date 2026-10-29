//! Where syncd reads and writes, resolved from the environment (PLAN §2.3): the one place the
//! daemon names a path, so a test can give it scratch directories. An account is named on disk
//! by its object-path segment (`porter_ids::object_segment`), the form the account's
//! directories are named in, so a wipe needs no map back to the id.

use ferry_engine::account::AccountDir;
use ferry_engine::rescan::Build;
pub use porter_daemon::xdg::PathError;
use porter_daemon::xdg::Xdg;
use std::path::PathBuf;

/// Every path the daemon uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// `$XDG_STATE_HOME/porter/sync`: one directory per account, one journal per dataset.
    pub journals: PathBuf,
    /// `$XDG_DATA_HOME/porter/vdir`: the PIM mirrors, one directory per account (W6e).
    pub mirrors: PathBuf,
    /// The system caller table, `/etc/porter/callers.toml` (another file in a test build:
    /// [`porter_callers::SystemCallers`]).
    pub callers_system: PathBuf,
    /// The user's caller table, `$XDG_CONFIG_HOME/porter/callers.toml`; its rows win.
    pub callers_user: PathBuf,
    /// `~/Cloud`: where each drive the person shows in Files is a folder (`~/Cloud/OneDrive`).
    pub cloud: PathBuf,
    /// `$XDG_CACHE_HOME/porter/drives`: each drive's downloaded files, one directory per account.
    pub drive_cache: PathBuf,
    /// `$XDG_CONFIG_HOME/porter/syncd/syncd.toml`: syncd's own settings (`storage.cache_limit`),
    /// in a folder of its own so that syncd's unit may write it and nothing else of porter's
    /// configuration (accountd's caller table sits in `porter/`). The file kept at
    /// `porter/syncd.toml` before is read while this one is absent.
    pub settings: PathBuf,
}

impl Paths {
    /// The paths for the environment `var` reads.
    pub fn resolve(var: impl Fn(&str) -> Option<String>) -> Result<Self, PathError> {
        let xdg = Xdg::new(var);
        let state = xdg.dir("XDG_STATE_HOME", ".local/state")?;
        let config = xdg.dir("XDG_CONFIG_HOME", ".config")?;
        let data = xdg.dir("XDG_DATA_HOME", ".local/share")?;
        let cache = xdg.dir("XDG_CACHE_HOME", ".cache")?;
        let home = xdg.home().ok_or(PathError::NoHome)?.to_path_buf();
        Ok(Self {
            journals: state.join("porter/sync"),
            mirrors: data.join("porter/vdir"),
            callers_system: PathBuf::from(porter_callers::SYSTEM_CALLERS),
            callers_user: config.join("porter/callers.toml"),
            cloud: home.join("Cloud"),
            drive_cache: cache.join("porter/drives"),
            settings: config.join("porter/syncd/syncd.toml"),
        })
    }

    /// The journal file of one dataset of one account.
    pub fn journal(&self, account: &AccountDir, dataset: &str) -> PathBuf {
        ferry_engine::journal::path(&self.journals, account, dataset)
    }

    /// The Photos library of one account, `$XDG_DATA_HOME/porter/photos/<account>` (W6f): beside
    /// the PIM mirrors, so `AccountRemoved` wipes it with them.
    pub fn photos_dir(&self, account: &AccountDir) -> PathBuf {
        self.mirrors
            .parent()
            .unwrap_or(&self.mirrors)
            .join("photos")
            .join(account.as_str())
    }

    /// The app folder mirror of one account, `$XDG_DATA_HOME/porter/storage/<account>`: beside
    /// the PIM mirrors, so `AccountRemoved` wipes it with them.
    pub fn storage_dir(&self, account: &AccountDir) -> PathBuf {
        self.mirrors
            .parent()
            .unwrap_or(&self.mirrors)
            .join("storage")
            .join(account.as_str())
    }

    /// The folder whose files syncd uploads to the account's Google Photos,
    /// `$XDG_DATA_HOME/porter/photos/<account>/upload`: inside the Photos library directory, so
    /// `AccountRemoved` wipes it with the rest.
    pub fn photos_upload_dir(&self, account: &AccountDir) -> PathBuf {
        self.photos_dir(account).join("upload")
    }

    /// Where the Google Photos picker puts what the person picked,
    /// `$XDG_DATA_HOME/porter/photos/<account>/picked`; one folder per session below it.
    pub fn photos_picked_dir(&self, account: &AccountDir) -> PathBuf {
        self.photos_dir(account).join("picked")
    }

    /// The ledger of what was uploaded to Google Photos (the album and every content hash sent),
    /// beside the account's journals so it goes with them.
    pub fn photos_ledger(&self, account: &AccountDir) -> PathBuf {
        self.journals
            .join(account.as_str())
            .join("google_photos_upload.ledger.json")
    }

    /// The downloaded files of one account's drive, `$XDG_CACHE_HOME/porter/drives/<account>`.
    pub fn drive_cache_dir(&self, account: &AccountDir) -> PathBuf {
        self.drive_cache.join(account.as_str())
    }

    /// Everything of one account that syncd keeps: its journals and its mirrors.
    pub fn account_dirs(&self, account: &AccountDir) -> [PathBuf; 2] {
        [
            self.journals.join(account.as_str()),
            self.mirrors.join(account.as_str()),
        ]
    }
}

/// The build this binary is.
pub const BUILD: Build = if cfg!(feature = "test-proc-root") {
    Build::Test
} else {
    Build::Release
};

/// Whether this binary honours `SYNCD_PROC_ROOT`: only with the `test-proc-root` feature. A
/// release build ignores the variable.
pub const HONOURS_PROC_ROOT: bool = cfg!(feature = "test-proc-root");

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn home_alone_gives_the_xdg_defaults() {
        let paths = Paths::resolve(env(&[("HOME", "/home/ada")])).expect("paths");
        assert_eq!(
            paths.journals,
            Path::new("/home/ada/.local/state/porter/sync")
        );
        assert_eq!(
            paths.mirrors,
            Path::new("/home/ada/.local/share/porter/vdir")
        );
        assert_eq!(
            paths.callers_user,
            Path::new("/home/ada/.config/porter/callers.toml")
        );
    }

    #[test]
    fn xdg_variables_win_and_relative_ones_are_ignored() {
        let paths = Paths::resolve(env(&[
            ("HOME", "/home/ada"),
            ("XDG_STATE_HOME", "/scratch/state"),
            ("XDG_DATA_HOME", "relative/data"),
        ]))
        .expect("paths");
        assert_eq!(paths.journals, Path::new("/scratch/state/porter/sync"));
        assert_eq!(
            paths.mirrors,
            Path::new("/home/ada/.local/share/porter/vdir")
        );
        assert_eq!(Paths::resolve(env(&[])), Err(PathError::NoHome));
    }

    #[test]
    fn a_photos_library_lives_beside_the_vdir_under_its_account() {
        let paths = Paths::resolve(env(&[("HOME", "/h")])).expect("paths");
        let account = AccountDir::parse("a1").expect("segment");
        assert_eq!(
            paths.photos_dir(&account),
            Path::new("/h/.local/share/porter/photos/a1")
        );
    }

    #[test]
    fn a_journal_lives_under_its_account_and_a_wipe_names_journals_and_mirrors() {
        let paths = Paths::resolve(env(&[("HOME", "/h")])).expect("paths");
        let account = AccountDir::parse("67e55044_10b1").expect("segment");
        assert_eq!(
            paths.journal(&account, "files"),
            Path::new("/h/.local/state/porter/sync/67e55044_10b1/files.sqlite")
        );
        let [journals, mirrors] = paths.account_dirs(&account);
        assert_eq!(
            journals,
            Path::new("/h/.local/state/porter/sync/67e55044_10b1")
        );
        assert_eq!(
            mirrors,
            Path::new("/h/.local/share/porter/vdir/67e55044_10b1")
        );
    }

    #[test]
    fn a_shipped_build_is_a_release_build() {
        let manifest = include_str!("../Cargo.toml");
        let default = manifest
            .lines()
            .skip_while(|l| !l.starts_with("[features]"))
            .any(|l| l.trim_start().starts_with("default"));
        assert!(
            !default,
            "no feature is default, so test-proc-root never is"
        );
    }
}
