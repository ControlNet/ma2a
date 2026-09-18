use std::collections::{BTreeMap, BTreeSet};

use iroh::endpoint::RelayMode;
use iroh_base::RelayUrl;
use iroh_relay::RelayMap;
use ma2a_core::{EndpointId, SpaceId};
use ma2a_store::ControlSpaceState;

use crate::{
    AdvertisementValidationContext, PrivateRelayAdvertisementValidator, PublicRelayFallbackConfig,
};

/// One provider identity and relay URL proven eligible by a current Space advertisement.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PrivateRelayCandidate {
    provider_endpoint_id: EndpointId,
    relay_url: RelayUrl,
}

impl PrivateRelayCandidate {
    /// Returns the Endpoint identity that signed the candidate advertisements.
    pub const fn provider_endpoint_id(&self) -> EndpointId {
        self.provider_endpoint_id
    }

    /// Returns the advertised relay URL.
    pub const fn relay_url(&self) -> &RelayUrl {
        &self.relay_url
    }
}

/// Safe desired relay candidates supplied to Iroh without selecting a home relay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalIrohRelayMap {
    active_spaces: BTreeSet<SpaceId>,
    coverage: BTreeMap<PrivateRelayCandidate, BTreeSet<SpaceId>>,
    eligible_private: BTreeSet<PrivateRelayCandidate>,
    home_private: BTreeSet<PrivateRelayCandidate>,
    public_relays: Vec<RelayUrl>,
    relay_urls: BTreeSet<RelayUrl>,
}

impl LocalIrohRelayMap {
    /// Filters current persisted advertisements against current Space authority and freshness.
    pub fn from_control_spaces(
        spaces: &[ControlSpaceState],
        public_fallback: Option<&PublicRelayFallbackConfig>,
        now_ms: u64,
    ) -> Self {
        let active_spaces = spaces
            .iter()
            .map(|state| state.authorization().space_id())
            .collect::<BTreeSet<_>>();
        let mut coverage = BTreeMap::<PrivateRelayCandidate, BTreeSet<SpaceId>>::new();
        for state in spaces {
            let authorization = state.authorization();
            for persisted in state.active_relay_advertisements() {
                let Ok(validated) = PrivateRelayAdvertisementValidator::validate(
                    persisted.signed_advertisement(),
                    AdvertisementValidationContext::new(&authorization, now_ms),
                ) else {
                    continue;
                };
                let advertisement = validated.signed().advertisement();
                let candidate = PrivateRelayCandidate {
                    provider_endpoint_id: advertisement.provider_endpoint_id(),
                    relay_url: advertisement.relay_url().clone(),
                };
                coverage
                    .entry(candidate)
                    .or_default()
                    .insert(advertisement.space_id());
            }
        }
        let eligible_private = coverage.keys().cloned().collect::<BTreeSet<_>>();
        let home_private = coverage
            .iter()
            .filter_map(|(candidate, covered_spaces)| {
                (!active_spaces.is_empty() && *covered_spaces == active_spaces)
                    .then(|| candidate.clone())
            })
            .collect::<BTreeSet<_>>();
        let public_relays =
            public_fallback.map_or_else(Vec::new, |fallback| fallback.relay_urls().to_vec());
        let relay_urls = home_private
            .iter()
            .map(|candidate| candidate.relay_url.clone())
            .chain(public_relays.iter().cloned())
            .collect();
        Self {
            active_spaces,
            coverage,
            eligible_private,
            home_private,
            public_relays,
            relay_urls,
        }
    }

    /// Returns the number of current valid Spaces used for compatibility filtering.
    pub fn active_space_count(&self) -> usize {
        self.active_spaces.len()
    }

    /// Returns whether a private relay is valid for at least one current Space.
    pub fn private_relay_eligible(
        &self,
        provider_endpoint_id: EndpointId,
        relay_url: &RelayUrl,
    ) -> bool {
        self.eligible_private.contains(&PrivateRelayCandidate {
            provider_endpoint_id,
            relay_url: relay_url.clone(),
        })
    }

    /// Returns whether one provider and URL has fresh coverage for every current Space.
    pub fn home_relay_compatible(
        &self,
        provider_endpoint_id: EndpointId,
        relay_url: &RelayUrl,
    ) -> bool {
        self.home_private.contains(&PrivateRelayCandidate {
            provider_endpoint_id,
            relay_url: relay_url.clone(),
        })
    }

    /// Iterates private candidates safe for Iroh's generic home-relay map.
    pub fn private_home_relays(&self) -> impl Iterator<Item = &PrivateRelayCandidate> {
        self.home_private.iter()
    }

    /// Iterates every fresh private candidate authorized by at least one current Space.
    pub fn private_relays(&self) -> impl Iterator<Item = &PrivateRelayCandidate> {
        self.eligible_private.iter()
    }

    /// Iterates every fresh private candidate with the exact Spaces it covers.
    ///
    /// `home_relay_compatible` is precisely "this set equals every active Space",
    /// so retaining it lets a client show the verdict and its reason together.
    pub fn private_coverage(
        &self,
    ) -> impl Iterator<Item = (&PrivateRelayCandidate, &BTreeSet<SpaceId>)> {
        self.coverage.iter()
    }

    /// Returns whether the URL identifies a private candidate compatible with every Space.
    pub fn is_private_home_relay(&self, relay_url: &RelayUrl) -> bool {
        self.home_private
            .iter()
            .any(|candidate| candidate.relay_url() == relay_url)
    }

    /// Returns whether the URL is an explicitly configured public fallback.
    pub fn is_public_relay(&self, relay_url: &RelayUrl) -> bool {
        self.public_relays.contains(relay_url)
    }

    /// Returns explicitly enabled public fallback URLs.
    pub fn public_relays(&self) -> &[RelayUrl] {
        &self.public_relays
    }

    /// Iterates every URL supplied to Iroh, with duplicates removed.
    pub fn relay_urls(&self) -> impl Iterator<Item = &RelayUrl> {
        self.relay_urls.iter()
    }

    /// Returns whether an Iroh-observed home URL belongs to the supplied candidate map.
    pub fn contains(&self, relay_url: &RelayUrl) -> bool {
        self.relay_urls.contains(relay_url)
    }

    /// Returns whether Iroh receives at least one private or public relay candidate.
    pub fn is_empty(&self) -> bool {
        self.relay_urls.is_empty()
    }

    /// Builds the exact URL-only Iroh relay map.
    pub fn relay_map(&self) -> RelayMap {
        self.relay_urls.iter().cloned().collect()
    }

    /// Builds disabled or custom Iroh relay mode without an implicit public fallback.
    pub fn relay_mode(&self) -> RelayMode {
        if self.is_empty() {
            RelayMode::Disabled
        } else {
            RelayMode::Custom(self.relay_map())
        }
    }
}
