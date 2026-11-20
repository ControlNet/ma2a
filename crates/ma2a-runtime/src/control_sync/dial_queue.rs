//! The requests of one control round waiting to be dialed.
//!
//! A peer sharing many Spaces receives several requests in one round. The responder admits at
//! most [`CONTROL_PER_PEER_LIMIT`] concurrent control streams per peer, so the queue never has
//! more in flight to one peer, and it remembers every peer that had a request fail: such a
//! peer was only partly synchronized.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use ma2a_core::EndpointId;
use ma2a_net::CONTROL_PER_PEER_LIMIT;

use super::PreparedControlPeer;

#[derive(Debug, Default)]
pub(super) struct DialQueue {
    pending: VecDeque<PreparedControlPeer>,
    in_flight: BTreeMap<EndpointId, usize>,
    failed: BTreeSet<EndpointId>,
}

impl DialQueue {
    pub(super) fn new(pending: impl IntoIterator<Item = PreparedControlPeer>) -> Self {
        Self {
            pending: pending.into_iter().collect(),
            ..Self::default()
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Takes the first waiting request whose peer still has room for another stream.
    pub(super) fn next(&mut self) -> Option<PreparedControlPeer> {
        let index = self.pending.iter().position(|prepared| {
            self.in_flight.get(&prepared.peer).copied().unwrap_or(0) < CONTROL_PER_PEER_LIMIT
        })?;
        let prepared = self.pending.remove(index)?;
        *self.in_flight.entry(prepared.peer).or_default() += 1;
        Some(prepared)
    }

    /// Records that one request to `peer` ended, and whether it succeeded.
    pub(super) fn finished(&mut self, peer: EndpointId, succeeded: bool) {
        if let Some(count) = self.in_flight.get_mut(&peer) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.in_flight.remove(&peer);
            }
        }
        if !succeeded {
            self.failed.insert(peer);
        }
    }

    /// Peers with at least one failed request this round.
    pub(super) const fn failed(&self) -> &BTreeSet<EndpointId> {
        &self.failed
    }
}

#[cfg(test)]
mod tests {
    use ma2a_net::{CONTROL_PER_PEER_LIMIT, EndpointSecret};

    use super::DialQueue;
    use crate::control_sync::PreparedControlPeer;

    #[test]
    fn one_peer_never_has_more_requests_in_flight_than_it_admits()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Given: three requests to one busy peer, then one to another peer.
        let busy = EndpointSecret::parse(&[0x61; 32])?.endpoint_id();
        let other = EndpointSecret::parse(&[0x62; 32])?.endpoint_id();
        let request = |peer| PreparedControlPeer {
            peer,
            request: Vec::new(),
        };
        let mut queue =
            DialQueue::new([request(busy), request(busy), request(busy), request(other)]);

        // When: everything that may start, starts.
        let started = std::iter::from_fn(|| queue.next())
            .map(|prepared| prepared.peer)
            .collect::<Vec<_>>();

        // Then: the third busy request waits, and the other peer is not held behind it.
        assert_eq!(
            started.iter().filter(|peer| **peer == busy).count(),
            CONTROL_PER_PEER_LIMIT
        );
        assert!(started.contains(&other));
        assert!(!queue.is_empty());
        queue.finished(busy, false);
        assert_eq!(queue.next().map(|prepared| prepared.peer), Some(busy));
        assert!(queue.is_empty());
        assert!(queue.failed().contains(&busy));
        Ok(())
    }
}
