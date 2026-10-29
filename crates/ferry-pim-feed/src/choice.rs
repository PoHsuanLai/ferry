//! What a daemon needs to choose a source for a grant: the transport the capability names, the
//! server a candidate lists, and the refusal when no source reads the pair. The choice itself
//! (which kinds a daemon runs, which sources it has) is the daemon's.

use ferry_vdir::PimKind;
use porter_capability::{Capability, PimTransport};
use porter_core::{Candidate, ServiceEndpoint};

/// Why no source reads a grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("no source reads {kind:?} over {transport:?} yet")]
pub struct NoSource {
    /// The kind.
    pub kind: PimKind,
    /// The capability's transport.
    pub transport: PimTransport,
}

/// The capability's transport for `kind`, or, for a candidate whose capability is not the kind's
/// (or an older bus peer), the kind's protocol family, which is what the mirror did before
/// sources existed.
pub fn transport_of(kind: PimKind, candidate: &Candidate) -> PimTransport {
    let named = match (kind, &candidate.capability) {
        (PimKind::Calendar, Capability::Calendar(cap))
        | (PimKind::Contacts, Capability::Contacts(cap))
        | (PimKind::Tasks, Capability::Tasks(cap)) => Some(cap.transport),
        _ => None,
    };
    named.unwrap_or(match kind {
        PimKind::Calendar => PimTransport::CalDav,
        PimKind::Contacts => PimTransport::CardDav,
        PimKind::Tasks => PimTransport::CalDav,
    })
}

/// The account's server of `family`, else its first.
pub fn endpoint_of(candidate: &Candidate, family: porter_core::Family) -> Option<&ServiceEndpoint> {
    candidate
        .endpoints
        .iter()
        .find(|e| e.family == family)
        .or_else(|| candidate.endpoints.first())
}
