use super::validate_display_name;

#[test]
fn display_name_uses_the_core_member_label_bound() {
    // Given
    let maximum = "m".repeat(ma2a_core::MAX_MEMBER_LABEL_LEN);
    let oversized = "m".repeat(ma2a_core::MAX_MEMBER_LABEL_LEN + 1);

    // When
    let maximum_result = validate_display_name(&maximum);
    let oversized_result = validate_display_name(&oversized);

    // Then
    assert!(maximum_result.is_ok());
    assert!(oversized_result.is_err());
}
