#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Principal {
    CurrentUser,
    System,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AccessRule {
    pub(super) principal: Principal,
    pub(super) allowed: bool,
    pub(super) inherited: bool,
    pub(super) full_control: bool,
}

pub(super) fn is_owner_only(owner: Principal, protected: bool, rules: &[AccessRule]) -> bool {
    if owner != Principal::CurrentUser || !protected || rules.len() != 2 {
        return false;
    }
    let mut user_seen = false;
    let mut system_seen = false;
    for rule in rules {
        if !rule.allowed || rule.inherited || !rule.full_control {
            return false;
        }
        match rule.principal {
            Principal::CurrentUser if !user_seen => user_seen = true,
            Principal::System if !system_seen => system_seen = true,
            Principal::CurrentUser | Principal::System | Principal::Other => return false,
        }
    }
    user_seen && system_seen
}

#[cfg(test)]
mod tests {
    use super::{AccessRule, Principal, is_owner_only};

    #[test]
    fn owner_with_extra_principal_is_not_owner_only() {
        // Given
        let rules = [
            AccessRule {
                principal: Principal::CurrentUser,
                allowed: true,
                inherited: false,
                full_control: true,
            },
            AccessRule {
                principal: Principal::System,
                allowed: true,
                inherited: false,
                full_control: true,
            },
            AccessRule {
                principal: Principal::Other,
                allowed: true,
                inherited: false,
                full_control: true,
            },
        ];

        // When
        let accepted = is_owner_only(Principal::CurrentUser, true, &rules);

        // Then
        assert!(!accepted);
    }

    #[test]
    fn protected_user_and_system_full_control_is_owner_only() {
        // Given
        let rules = [
            AccessRule {
                principal: Principal::CurrentUser,
                allowed: true,
                inherited: false,
                full_control: true,
            },
            AccessRule {
                principal: Principal::System,
                allowed: true,
                inherited: false,
                full_control: true,
            },
        ];

        // When
        let accepted = is_owner_only(Principal::CurrentUser, true, &rules);

        // Then
        assert!(accepted);
    }

    #[test]
    fn inherited_owner_rules_are_not_owner_only() {
        // Given
        let rules = [
            AccessRule {
                principal: Principal::CurrentUser,
                allowed: true,
                inherited: true,
                full_control: true,
            },
            AccessRule {
                principal: Principal::System,
                allowed: true,
                inherited: false,
                full_control: true,
            },
        ];

        // When
        let accepted = is_owner_only(Principal::CurrentUser, true, &rules);

        // Then
        assert!(!accepted);
    }
}
