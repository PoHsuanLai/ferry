# ferry

Ferry keeps the copies of your calendars, contacts, tasks, files and photos on your computer in
step with the services your accounts reach, and shows an account's whole drive as a folder
(`~/Cloud/OneDrive`, `~/Cloud/Google Drive`) whose files come down when they are opened. It is the
sync half of what began as porter; [porter](https://github.com/PoHsuanLai/porter) is the accounts
half (sign-in, consent, the keyring), and ferry depends on it by git rev.

## What runs

| Service | Name on the session bus | What it does |
| --- | --- | --- |
| `syncd` | `org.quire.Sync1`, `org.quire.Storage1` | Keeps the copies in step for the apps that hold a grant; "Sync now" brings one up to date at once. Serves the drives shown in Files. |
| `porter-mountd` | none (a socket) | A small user service started by its socket: it runs the system's `fusermount3` to show a drive at `~/Cloud/<name>` and hands syncd the open FUSE connection, because syncd's sandbox refuses the mount. |

## Crates

| Crate | What it is |
| --- | --- |
| `ferry-sync` | the sync contract: `Replica`, anchors, base versions, conflicts, tombstones, and an in-memory replica |
| `storage-webdav`, `storage-graph`, `storage-gdrive` | the replicas: WebDAV, OneDrive's Graph and Google Drive |
| `ferry-drive`, `ferry-drive-fuse`, `ferry-mountd` | files on demand over a drive's listing, the FUSE adapter and the mount helper |
| `ferry-bus` | the `Sync1`, `Storage1` and Photos Picker interfaces |
| `syncd` | the daemon: the journal, the engine, the scheduler, the datasets (calendars, contacts, tasks, the storage folder, whole drives, photos) and the bus objects |
| `quickxorhash`, `windows-zones` | OneDrive's hash and the Windows-to-IANA time zone table |
| `syncd-fakes`, `ferry-fake-graph`, `ferry-rig` | test tooling, never installed |

[ARCHITECTURE.md](ARCHITECTURE.md) has the edges between them.

## Accounts

The accounts are accountd's (porter). ferry reaches them only through porter's client crates
(`porter-client` over `porter-client-dbus`): syncd lists the accounts, asks accountd for the grant
it needs (accountd's consent sheet shows the first time a sync switch is turned on for an account,
and its answer is kept), reads its own grants, and opens the streams to an account's servers that
accountd has signed in. syncd holds no account, no token and no password of its own, and
`scripts/check-boundary.sh` fails if a crate here builds on porter's account stack.

## Install

You need a Linux desktop with systemd, a keyring (GNOME Keyring or KWallet), `fuse3` for the drives
and a Rust toolchain to build. Install porter first (it ships the caller table that names syncd).

    scripts/install.sh                      # builds, then installs under /usr (run as root)
    scripts/install.sh --prefix /usr/local
    scripts/install.sh --destdir /tmp/stage # copies into a staging folder and touches nothing else

It can be run again at any time and never writes into a home folder. `--bin-dir DIR` installs
binaries you built yourself, and `--help` lists the rest. [docs/drive-files.md](docs/drive-files.md)
checks the real folder by hand.

## Working on ferry

    cargo test --workspace        # the tests that start a private session bus are meant to run in a jail; .github/workflows/ci.yml says which packages
    scripts/check-boundary.sh     # the crate boundaries, mechanically
    scripts/check-features.sh     # the feature sets the programs build in, each warning-free
    cargo deny check licenses

## Licence

MIT or Apache-2.0, at your option: [LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE).
