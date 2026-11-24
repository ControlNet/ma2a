#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Principal {
    CurrentUser,
    System,
    CurrentUserAndSystem,
    Administrators,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AccessRule {
    pub(super) principal: Principal,
    pub(super) allowed: bool,
    pub(super) inherited: bool,
    pub(super) full_control: bool,
}

/// Whether an object is owned by a trusted principal and its protected DACL holds only explicit
/// Full Control grants that together cover the current user and SYSTEM.
///
/// When the process runs as SYSTEM the two are one principal, which Windows may record as one
/// rule or two, so one or two rules are accepted, each for the user or SYSTEM.
pub(super) fn is_owner_only(owner: Principal, protected: bool, rules: &[AccessRule]) -> bool {
    let trusted_owner = matches!(
        owner,
        Principal::CurrentUser
            | Principal::System
            | Principal::CurrentUserAndSystem
            | Principal::Administrators
    );
    if !trusted_owner || !protected || !(1..=2).contains(&rules.len()) {
        return false;
    }
    let mut user_seen = false;
    let mut system_seen = false;
    for rule in rules {
        if !rule.allowed || rule.inherited || !rule.full_control {
            return false;
        }
        match rule.principal {
            Principal::CurrentUser => user_seen = true,
            Principal::System => system_seen = true,
            Principal::CurrentUserAndSystem => (user_seen, system_seen) = (true, true),
            Principal::Administrators | Principal::Other => return false,
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

    #[test]
    fn administrators_owner_and_local_system_self_rules_are_accepted() {
        let user_and_system = [
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
        assert!(is_owner_only(
            Principal::Administrators,
            true,
            &user_and_system
        ));
        let admins_in_dacl = [
            user_and_system[0],
            AccessRule {
                principal: Principal::Administrators,
                allowed: true,
                inherited: false,
                full_control: true,
            },
        ];
        assert!(!is_owner_only(
            Principal::CurrentUser,
            true,
            &admins_in_dacl
        ));
        let local_system = [
            AccessRule {
                principal: Principal::CurrentUserAndSystem,
                allowed: true,
                inherited: false,
                full_control: true,
            },
            AccessRule {
                principal: Principal::CurrentUserAndSystem,
                allowed: true,
                inherited: false,
                full_control: true,
            },
        ];
        assert!(is_owner_only(
            Principal::CurrentUserAndSystem,
            true,
            &local_system
        ));
        // Windows may also merge the grant to SYSTEM-as-user into one rule.
        assert!(is_owner_only(
            Principal::CurrentUserAndSystem,
            true,
            &local_system[..1]
        ));
        // A lone grant to the user, or one repeated, never covers SYSTEM.
        let user_only = [user_and_system[0], user_and_system[0]];
        assert!(!is_owner_only(Principal::CurrentUser, true, &user_only));
        assert!(!is_owner_only(
            Principal::CurrentUser,
            true,
            &user_only[..1]
        ));
    }
}
