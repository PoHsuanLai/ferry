//! The one knob a release must not have: how often the supervisors read grants again, which a
//! test build shortens so a scenario need not wait the default ten minutes for a grant it just
//! made.

use std::time::Duration;

/// Which build a daemon is, for the knobs a release must not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Build {
    /// Built with the daemon's `test-proc-root` feature.
    Test,
    /// Every other build: the shipped one.
    Release,
}

/// How often the supervisors read grants again (a daemon's `..._RESCAN_S` variable, whole
/// seconds, at least 1), for a test build only. A release build ignores the variable; so does a
/// value that is not a number of seconds. The default is `default`, and no build has a shorter
/// one.
pub fn rescan(build: Build, value: Option<String>, default: Duration) -> Duration {
    match build {
        Build::Test => value
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|seconds| *seconds >= 1)
            .map_or(default, Duration::from_secs),
        Build::Release => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rescan_variable_is_read_by_a_test_build_only_and_never_shortens_the_default() {
        let default = Duration::from_secs(600);
        let cases = [
            (Build::Test, Some("2"), Duration::from_secs(2)),
            (Build::Test, Some(" 5 "), Duration::from_secs(5)),
            (Build::Test, Some("0"), default),
            (Build::Test, Some("soon"), default),
            (Build::Test, Some("-3"), default),
            (Build::Test, None, default),
            (Build::Release, Some("2"), default),
            (Build::Release, None, default),
        ];
        for (build, value, want) in cases {
            assert_eq!(
                rescan(build, value.map(str::to_owned), default),
                want,
                "{build:?} {value:?}"
            );
        }
    }
}
