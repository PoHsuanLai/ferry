//! What every HTTP source writes and reads the same way: a path segment made of an id, the
//! file name an item gets from its id, and what a failed request means to a replica. Pure.

use ferry_sync::{ItemPath, ReplicaError, RetryAfter};
use porter_http::http::Response;
use porter_http::{HeaderText, HttpError};
use porter_ids::UnixSeconds;
use sha2::{Digest, Sha256};

/// What a Graph or Google id keeps in a path segment: the unreserved characters and `=`, which
/// the ids' base64 padding uses.
const ID_SEGMENT: &porter_ids::percent::AsciiSet = &porter_ids::percent::UNRESERVED.remove(b'=');

/// A path segment: everything but unreserved characters and `=` percent-encoded.
pub fn encode(segment: &str) -> String {
    porter_ids::percent::encode(segment, ID_SEGMENT)
}

/// The id a path segment was made from.
pub fn decode(segment: &str) -> String {
    porter_ids::percent::decode(segment)
}

/// The file name of an event: short and safe whatever its id.
pub fn name_of(id: &str) -> ItemPath {
    ItemPath(format!(
        "{}.ics",
        hex::encode(&Sha256::digest(id.as_bytes())[..10])
    ))
}

/// How long a replica waits to ask again after `response`: its `Retry-After`, else 30 seconds.
pub fn retry_after(response: &Response<Vec<u8>>, now: UnixSeconds) -> RetryAfter {
    response
        .retry_after_seconds(now)
        .map_or(RetryAfter(30), RetryAfter)
}

/// A request that did not reach the server is transient.
pub fn unreached(_: HttpError) -> ReplicaError {
    ReplicaError::Transient(RetryAfter(30))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_segment_round_trips_through_encoding() {
        for id in ["AAMk=", "a/b c", "x+y%z", "é"] {
            assert_eq!(decode(&encode(id)), id);
        }
        assert_eq!(encode("a/b"), "a%2Fb");
    }

    #[test]
    fn a_file_name_is_short_stable_and_safe_whatever_the_id() {
        let id = "AAMkAGI2TG93AAA=/with+odd/chars";
        let name = name_of(id);
        assert_eq!(name, name_of(id));
        assert_ne!(name, name_of("other"));
        assert!(name.0.ends_with(".ics") && name.0.len() == 24, "{name:?}");
        assert!(
            name.0
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.')
        );
    }
}
