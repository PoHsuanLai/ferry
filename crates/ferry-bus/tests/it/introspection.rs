//! The checked-in introspection files are the interfaces as they are declared once. A change to a
//! signature changes the file in the same commit.

use std::path::PathBuf;

#[test]
fn checked_in_introspection_matches_the_interfaces() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../dbus");
    for document in [ferry_bus::SYNC, ferry_bus::STORAGE] {
        let path = dir.join(document.file_name);
        let expected =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let actual = match document.file_name {
            "org.quire.Sync1.xml" => without_picker(&document.introspection()),
            _ => document.introspection(),
        };
        assert!(
            actual == expected,
            "{} differs from the interfaces; it should read:\n{actual}",
            path.display()
        );
    }
}

/// The Sync1 document without the Photos Picker, which is outside the default surface (the
/// `photos-picker` feature adds it, and the test below it checks it then).
fn without_picker(xml: &str) -> String {
    match xml.split_once("<interface name=\"org.quire.Photos1.Picker\">") {
        Some((sync1, _)) => format!("{}</node>\n", sync1.trim_end_matches(' ')),
        None => xml.to_owned(),
    }
}

#[test]
fn syncd_declares_what_it_did() {
    let sync = ferry_bus::SYNC.introspection();
    let sync1 = without_picker(&sync);
    assert_eq!(sync1.matches("<method ").count(), 8);
    assert_eq!(sync1.matches("<signal ").count(), 3);
    assert!(sync1.contains("<method name=\"ConfirmDiscard\">"));
    assert!(sync1.contains("<method name=\"SyncNow\">"));
    assert_eq!(
        ferry_bus::SYNC_ERROR_PAUSED,
        format!("{}Paused", ferry_bus::SYNC_ERROR_PREFIX)
    );
    assert!(sync1.contains("<method name=\"Watch\">"));
    assert!(sync1.contains("<signal name=\"NeedsConfirmation\">"));
    assert_eq!(
        ferry_bus::SYNC_ERROR_NOTHING_HELD,
        format!("{}NothingHeld", ferry_bus::SYNC_ERROR_PREFIX)
    );
    #[cfg(feature = "photos-picker")]
    {
        let (_, picker) = sync
            .split_once("<interface name=\"org.quire.Photos1.Picker\">")
            .expect("the Picker interface is declared beside Sync1");
        for member in ["Start", "Poll", "Import", "Cancel"] {
            assert!(
                picker.contains(&format!("<method name=\"{member}\">")),
                "{member}"
            );
        }
        assert_eq!(picker.matches("<method ").count(), 4);
        assert_eq!(picker.matches("<signal ").count(), 0);
        assert!(
            picker.contains("<arg type=\"as\" direction=\"out\"/>"),
            "Import answers the files' paths"
        );
        assert_eq!(
            ferry_bus::PICKER_ERROR_NOT_YET,
            format!("{}NotYet", ferry_bus::PICKER_ERROR_PREFIX)
        );
    }
    #[cfg(not(feature = "photos-picker"))]
    assert!(
        !sync.contains("org.quire.Photos1"),
        "the Picker is not declared by default"
    );
    assert!(sync.contains("<method name=\"Resolve\">"));
    assert!(sync.contains("<arg name=\"conflict\" type=\"x\" direction=\"in\"/>"));
    assert_eq!(ferry_bus::RESOLVE_KEEP_LOCAL, "keep_local");
    assert_eq!(ferry_bus::RESOLVE_KEEP_REMOTE, "keep_remote");
    assert_eq!(
        ferry_bus::SYNC_ERROR_NO_SUCH_CONFLICT,
        format!("{}NoSuchConflict", ferry_bus::SYNC_ERROR_PREFIX)
    );
    assert_eq!(ferry_bus::STATUS_KEY_QUOTA, "quota");
}
