//! An account as the engine's files name it.

/// An account as a directory name: the `[A-Za-z0-9_]+` segment of its object path
/// (`porter_ids::object_segment`), the form the account's directories are named in, so a wipe
/// needs no map back to the id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AccountDir(String);

impl AccountDir {
    /// The segment, if it is one: nothing else may become a path component.
    pub fn parse(segment: &str) -> Option<Self> {
        let ok = !segment.is_empty()
            && segment.len() <= 64
            && segment
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_');
        ok.then(|| Self(segment.to_owned()))
    }

    /// The directory name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AccountDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_plain_segment_becomes_a_directory_name() {
        const CASES: &[(&str, bool)] = &[
            ("abc_123", true),
            ("", false),
            ("..", false),
            ("a/b", false),
            ("a-b", false),
            ("a.b", false),
        ];
        for (text, ok) in CASES {
            assert_eq!(AccountDir::parse(text).is_some(), *ok, "{text:?}");
        }
    }
}
