/// A signed relay publication is current only before its five-minute renewal
/// boundary and while more than five minutes remain before expiry.
pub(crate) const fn is_current(now_ms: u64, issued_at_ms: u64, expires_at_ms: u64) -> bool {
    const RENEWAL_MS: u64 = 300_000;
    now_ms >= issued_at_ms
        && now_ms - issued_at_ms < RENEWAL_MS
        && now_ms.saturating_add(RENEWAL_MS) < expires_at_ms
}
