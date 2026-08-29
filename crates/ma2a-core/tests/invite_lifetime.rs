//! Enrollment invitation lifetime invariants.

use ma2a_core::InviteValidity;

#[test]
fn invite_lifetime_over_five_minutes_is_rejected() {
    // Given
    let created_at_ms = 1_000;

    // When
    let result = InviteValidity::new(created_at_ms, 301_001);

    // Then
    assert!(result.is_err());
}
