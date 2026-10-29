//! The datasets syncd mirrors into a vdir: address books from a CardDAV server or Google
//! (design/31 §6.2). The mirror, the vdir and the supervisor are libraries (`ferry-pim-mirror`,
//! `ferry-vdir`); what is syncd's is which source reads a grant ([`ContactSources`]). Calendars
//! and task lists are callie's calendard, not syncd's.
//!
//! The source is chosen by the capability's transport ([`choose`]: `carddav` is DAV,
//! `google_api` is [`google`]), never by provider.
//!
//! **A new source is one module**: a `PimSource` and a `Feed`, one more arm in [`choose`], one
//! more arm in `ContactSources::refresh`. Nothing else is touched.

pub mod google;

use ferry_pim_feed::{NoSource, transport_of};
use ferry_pim_mirror::{AccountMirrors, DavSource, RefreshError, Sources, Wiring};
use ferry_vdir::PimKind;
use google::GooglePeopleSource;
use porter_capability::PimTransport;
use porter_client::GrantsLink;
use porter_core::Candidate;
use std::sync::Arc;

/// Which source an account's grant for contacts is read through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chosen {
    /// CardDAV.
    Dav,
    /// Google contacts (the People API).
    GooglePeople,
}

/// The source for the account's capability: by its transport, never by provider name.
pub fn choose(candidate: &Candidate) -> Result<Chosen, NoSource> {
    let kind = PimKind::Contacts;
    let transport = transport_of(kind, candidate);
    match transport {
        PimTransport::CalDav | PimTransport::CardDav => Ok(Chosen::Dav),
        PimTransport::GoogleApi => Ok(Chosen::GooglePeople),
        transport => Err(NoSource { kind, transport }),
    }
}

/// syncd's sources: address books, each read by the source its grant's transport names.
#[derive(Debug, Clone, Copy)]
pub struct ContactSources;

impl<T: GrantsLink + 'static> Sources<T> for ContactSources {
    const NAME: &'static str = "syncd";
    const KINDS: &'static [PimKind] = &[PimKind::Contacts];

    async fn refresh(
        &self,
        mirrors: &mut AccountMirrors,
        wiring: &Wiring<T>,
        candidate: &Candidate,
    ) -> Result<(), RefreshError> {
        let kind = mirrors.kind();
        let chosen = choose(candidate)?;
        let family = match chosen {
            Chosen::Dav => kind.family(),
            Chosen::GooglePeople => porter_core::Family::GooglePeople,
        };
        // A Google API is reached at its own server only: the first of the account's other
        // endpoints (its IMAP host, say) is never the contacts'.
        let own_only = chosen == Chosen::GooglePeople;
        let endpoint = ferry_pim_feed::endpoint_of(candidate, family)
            .filter(|e| !own_only || e.family == family)
            .ok_or(RefreshError::NoEndpoint(kind))?
            .url
            .clone();
        let accounts = Arc::clone(&wiring.accounts);
        let grant = candidate.grant.clone();
        match chosen {
            Chosen::Dav => {
                let source = DavSource::new(
                    accounts,
                    grant,
                    endpoint,
                    wiring.clock.clone(),
                    wiring.nonce.clone(),
                );
                mirrors.refresh_with(wiring, source).await
            }
            Chosen::GooglePeople => {
                let source =
                    GooglePeopleSource::new(accounts, grant, endpoint, wiring.clock.clone())?;
                mirrors.refresh_with(wiring, source).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferry_pim_feed::FeedCursor;
    use ferry_pim_mirror::DavCursor;
    use porter_capability::Subject;
    use porter_capability::{Access, Capability, Delta, PimCap};
    use porter_core::{AccountId, GrantId, ProviderId};
    use porter_core::{
        AccountLabel, EndpointUrl, Family, LoginName, Restriction, ServiceEndpoint, Tls,
    };

    fn cap(transport: PimTransport) -> PimCap {
        PimCap {
            access: Access::Read,
            delta: Delta::Poll,
            transport,
            collections: true,
        }
    }

    fn candidate(provider: &str, capability: Capability) -> Candidate {
        Candidate::new(
            AccountId::parse("a1").expect("account"),
            AccountLabel("a@b.test".into()),
            ProviderId::parse(provider).expect("provider"),
            Subject::Account,
            capability,
            Restriction::none(),
            GrantId::parse("g1").expect("grant"),
        )
        .with_endpoints(vec![ServiceEndpoint {
            family: Family::GooglePeople,
            url: EndpointUrl::parse("https://people.example.test").expect("url"),
            tls: Tls::Plain,
            login: LoginName("a".into()),
        }])
    }

    #[test]
    fn the_source_is_chosen_by_the_capabilitys_transport_never_by_the_provider() {
        use PimTransport as P;
        let contacts = |t| Capability::Contacts(cap(t));
        // The same provider name with each transport; a provider called "microsoft" or "google" is
        // not special.
        for provider in ["microsoft", "google", "anything"] {
            let cases = [
                (contacts(P::CardDav), Ok(Chosen::Dav)),
                (contacts(P::GoogleApi), Ok(Chosen::GooglePeople)),
                (
                    contacts(P::Graph),
                    Err(NoSource {
                        kind: PimKind::Contacts,
                        transport: P::Graph,
                    }),
                ),
                (
                    contacts(P::Jmap),
                    Err(NoSource {
                        kind: PimKind::Contacts,
                        transport: P::Jmap,
                    }),
                ),
            ];
            for (capability, want) in cases {
                let got = choose(&candidate(provider, capability.clone()));
                assert_eq!(got, want, "{provider} {capability:?}");
            }
        }
    }

    #[test]
    fn a_cursor_text_that_is_not_the_sources_is_not_a_cursor() {
        assert_eq!(
            DavCursor::decode("sync-token-1")
                .map(|c| c.encode())
                .as_deref(),
            Some("sync-token-1")
        );
    }
}
