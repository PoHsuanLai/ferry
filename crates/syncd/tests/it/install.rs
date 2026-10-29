//! `scripts/install.sh`: run into a staging directory (`--destdir`) with stand-in binaries, it puts
//! every file the units and the activation file name where they say, again changes nothing, and
//! writes nothing under a home directory.

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use test_kit::Scratch;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Stand-in binaries: the script installs what it is given, it does not need real builds.
fn stand_ins(root: &Path) -> PathBuf {
    let bins = root.join("bins");
    fs::create_dir_all(&bins).expect("bins");
    for daemon in ["syncd", "porter-mountd"] {
        let file = bins.join(daemon);
        fs::write(&file, format!("#!/bin/sh\necho {daemon}\n")).expect("write");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    bins
}

struct Run {
    destdir: PathBuf,
    home: PathBuf,
    bins: PathBuf,
    /// Removes the staging area when the run ends; last so nothing outlives it.
    _root: Scratch,
}

impl Run {
    fn new(name: &str) -> Run {
        let root = Scratch::new(&format!("install-{name}"));
        let home = root.join("home");
        fs::create_dir_all(&home).expect("home");
        Run {
            destdir: root.join("stage"),
            bins: stand_ins(&root),
            home,
            _root: root,
        }
    }

    /// The script's standard output.
    fn install(&self, extra: &[&str]) -> String {
        let out = Command::new("sh")
            .arg(repo().join("scripts/install.sh"))
            .args(["--bin-dir"])
            .arg(&self.bins)
            .arg("--destdir")
            .arg(&self.destdir)
            .args(extra)
            .env("HOME", &self.home)
            .env_remove("PREFIX")
            .env_remove("DESTDIR")
            .output()
            .expect("run the script");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).expect("utf-8")
    }

    fn at(&self, path: &str) -> PathBuf {
        self.destdir.join(path.trim_start_matches('/'))
    }
}

fn files_under(dir: &Path) -> BTreeSet<PathBuf> {
    let mut found = BTreeSet::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(next) = pending.pop() {
        let Ok(entries) = fs::read_dir(&next) else {
            continue;
        };
        for entry in entries {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                found.insert(path);
            }
        }
    }
    found
}

fn value(file: &str, key: &str) -> Option<String> {
    file.lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
        .map(str::to_owned)
}

/// The executable of an `ExecStart=` line: systemd's `-`, `+`, `!` and `@` prefixes dropped.
fn program(line: &str) -> &str {
    let line = line.trim_start_matches(['-', '+', '!', '@', ':']);
    line.split_whitespace().next().unwrap_or("")
}

#[test]
fn every_file_a_unit_or_an_activation_file_names_is_installed_at_the_path_it_names() {
    for prefix in ["/usr", "/opt/ferry-try"] {
        let run = Run::new(&format!("names{}", prefix.replace('/', "-")));
        run.install(&["--prefix", prefix]);
        let units = run.at(&format!("{prefix}/lib/systemd/user"));
        let mut checked = 0;
        for unit in ["syncd", "porter-mountd"] {
            let text = fs::read_to_string(units.join(format!("{unit}.service"))).expect("unit");
            let exec = value(&text, "ExecStart").expect("ExecStart");
            assert_eq!(program(&exec), format!("{prefix}/libexec/quire/{unit}"));
            assert!(run.at(program(&exec)).is_file(), "{exec}");
            checked += 1;
        }
        let activations = files_under(&run.at(&format!("{prefix}/share/dbus-1/services")));
        assert_eq!(activations.len(), 1);
        for path in activations {
            let text = fs::read_to_string(&path).expect("activation");
            let exec = value(&text, "Exec").expect("Exec");
            assert!(run.at(&exec).is_file(), "{} names {exec}", path.display());
            let service = value(&text, "SystemdService").expect("SystemdService");
            let unit = fs::read_to_string(units.join(&service)).expect("its unit is installed");
            assert_eq!(value(&unit, "BusName"), value(&text, "Name"));
            assert_eq!(value(&unit, "ExecStart"), Some(exec));
            checked += 1;
        }
        assert_eq!(checked, 3);
    }
}

#[test]
fn the_binaries_are_executable_and_the_units_are_not() {
    let run = Run::new("modes");
    run.install(&[]);
    let mode = |path: &str| {
        fs::metadata(run.at(path))
            .expect("meta")
            .permissions()
            .mode()
            & 0o777
    };
    assert_eq!(mode("/usr/libexec/quire/syncd"), 0o755);
    assert_eq!(mode("/usr/libexec/quire/porter-mountd"), 0o755);
    assert_eq!(mode("/usr/lib/systemd/user/syncd.service"), 0o644);
}

#[test]
fn a_second_run_changes_nothing_and_a_home_directory_is_never_written() {
    let run = Run::new("again");
    let first = run.install(&[]);
    assert!(
        first.contains("installed /usr/libexec/quire/syncd"),
        "{first}"
    );
    let before: Vec<_> = files_under(&run.destdir)
        .into_iter()
        .map(|p| {
            let modified = fs::metadata(&p).expect("meta").modified().expect("mtime");
            (p, modified)
        })
        .collect();
    let second = run.install(&[]);
    assert_eq!(second.trim(), "nothing to change");
    let after: Vec<_> = files_under(&run.destdir)
        .into_iter()
        .map(|p| {
            let modified = fs::metadata(&p).expect("meta").modified().expect("mtime");
            (p, modified)
        })
        .collect();
    assert_eq!(before, after);
    assert!(files_under(&run.home).is_empty());
    // Everything is under the prefix, and nothing is left staged.
    for path in files_under(&run.destdir) {
        let rel = path.strip_prefix(&run.destdir).expect("under destdir");
        assert!(rel.starts_with("usr"), "{rel:?}");
        assert!(!path.to_string_lossy().contains(".new."), "{path:?}");
    }
}

#[test]
fn no_unit_has_a_live_drop_in_but_the_sandbox_one() {
    let run = Run::new("tailnet");
    run.install(&["--prefix", "/opt/ferry"]);
    // Nothing is written under the home folder, and no unit has a drop-in but the sandbox one.
    assert!(files_under(&run.home).is_empty());
    for path in files_under(&run.destdir) {
        let live = path
            .components()
            .any(|part| part.as_os_str().to_string_lossy().ends_with(".service.d"));
        assert!(
            !live || path.file_name().is_some_and(|name| name == "sandbox.conf"),
            "{path:?}"
        );
    }
    assert!(!run.at("/etc/systemd").exists());
}

#[test]
fn the_sandbox_drop_in_lands_next_to_its_unit_without_a_probe_when_staging() {
    let run = Run::new("sandbox");
    // With a destdir the script does not ask this computer whether it allows mount namespaces.
    let out = run.install(&["--prefix", "/opt/ferry"]);
    assert!(!out.contains("without their extra sandbox"), "{out}");
    let installed = run.at("/opt/ferry/lib/systemd/user/syncd.service.d/sandbox.conf");
    assert_eq!(
        fs::read(&installed).expect("the drop-in is installed"),
        fs::read(repo().join("dist/syncd.service.d/sandbox.conf")).expect("shipped")
    );
    assert_eq!(
        fs::metadata(&installed).expect("meta").permissions().mode() & 0o777,
        0o644
    );
    assert!(files_under(&run.home).is_empty());
}

#[test]
fn the_drive_folder_helper_is_installed_with_its_socket_enabled_for_everyone() {
    let run = Run::new("mountd");
    run.install(&["--prefix", "/opt/ferry"]);
    let units = run.at("/opt/ferry/lib/systemd/user");
    for unit in ["porter-mountd.service", "porter-mountd.socket"] {
        assert!(units.join(unit).is_file(), "{unit}");
    }
    // Enabled as a package enables it: a link beside the units, nothing under /etc or a home.
    let wants = units.join("sockets.target.wants/porter-mountd.socket");
    assert_eq!(
        fs::read_link(&wants).expect("the socket is enabled"),
        Path::new("../porter-mountd.socket")
    );
    assert!(wants.is_file(), "the link resolves to the socket unit");
    assert!(!run.at("/etc/systemd").exists());
    assert!(files_under(&run.home).is_empty());
    // It has no sandbox drop-in: its unit says what it may not have.
    assert!(!units.join("porter-mountd.service.d").exists());
    let again = run.install(&["--prefix", "/opt/ferry"]);
    assert_eq!(again.trim(), "nothing to change");
}

#[test]
fn the_script_builds_the_daemon_and_the_helper_with_default_features_only() {
    let script = fs::read_to_string(repo().join("scripts/install.sh")).expect("script");
    let builds: Vec<_> = script
        .lines()
        .filter(|l| l.contains("cargo build"))
        .collect();
    assert_eq!(builds.len(), 1, "{builds:?}");
    assert!(!builds[0].contains("feature"), "{}", builds[0]);
    assert!(builds[0].contains("--release"));
    assert!(script.contains("-p syncd -p ferry-mountd"));
    assert!(!script.contains("test-proc-root"));
}
