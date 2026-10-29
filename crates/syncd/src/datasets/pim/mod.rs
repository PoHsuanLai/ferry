//! The datasets syncd mirrors into a vdir: calendars, address books and task lists from a
//! CalDAV/CardDAV server, Microsoft Graph or Google (design/31 §6.2). The mirror, the vdir and
//! the supervisor are libraries (`ferry-pim-mirror`, `ferry-vdir`); what is syncd's is which
//! source reads a grant ([`SyncdSources`]).

pub mod source;

use ferry_pim_mirror::{AccountMirrors, DavSource, RefreshError, Sources, Wiring};
use ferry_vdir::PimKind;
use porter_client::GrantsLink;
use porter_core::Candidate;
use source::google::{GoogleCalendarSource, GooglePeopleSource, GoogleTasksSource};
use source::graph::GraphCalendarSource;
use source::{Chosen, choose};
use std::sync::Arc;

/// syncd's sources: every kind, each read by the source its grant's transport names.
#[derive(Debug, Clone, Copy)]
pub struct SyncdSources;

impl<T: GrantsLink + 'static> Sources<T> for SyncdSources {
    const NAME: &'static str = "syncd";
    const KINDS: &'static [PimKind] = &PimKind::ALL;

    async fn refresh(
        &self,
        mirrors: &mut AccountMirrors,
        wiring: &Wiring<T>,
        candidate: &Candidate,
    ) -> Result<(), RefreshError> {
        let kind = mirrors.kind();
        let chosen = choose(kind, candidate)?;
        let family = match chosen {
            Chosen::Dav => kind.family(),
            Chosen::GraphCalendar => porter_core::Family::Graph,
            Chosen::GoogleCalendar => porter_core::Family::GoogleCalendar,
            Chosen::GooglePeople => porter_core::Family::GooglePeople,
            Chosen::GoogleTasks => porter_core::Family::GoogleTasks,
        };
        // A Google API is reached at its own server only: the first of the account's other
        // endpoints (its IMAP host, say) is never the calendar's.
        let own_only = matches!(
            chosen,
            Chosen::GoogleCalendar | Chosen::GooglePeople | Chosen::GoogleTasks
        );
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
            Chosen::GraphCalendar => {
                let source =
                    GraphCalendarSource::new(accounts, grant, endpoint, wiring.clock.clone())?;
                mirrors.refresh_with(wiring, source).await
            }
            Chosen::GoogleCalendar => {
                let source =
                    GoogleCalendarSource::new(accounts, grant, endpoint, wiring.clock.clone())?;
                mirrors.refresh_with(wiring, source).await
            }
            Chosen::GooglePeople => {
                let source =
                    GooglePeopleSource::new(accounts, grant, endpoint, wiring.clock.clone())?;
                mirrors.refresh_with(wiring, source).await
            }
            Chosen::GoogleTasks => {
                let source =
                    GoogleTasksSource::new(accounts, grant, endpoint, wiring.clock.clone())?;
                mirrors.refresh_with(wiring, source).await
            }
        }
    }
}
