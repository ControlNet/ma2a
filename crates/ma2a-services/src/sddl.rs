//! Deciding from a security descriptor's SDDL text whether an object is private to one user.

/// Whether `sddl`, an owner followed by a DACL as Windows formats them, admits nobody but
/// `user` (a SID in `S-1-…` form), SYSTEM, and the Administrators group.
///
/// The owner must be one of those three, since an owner may rewrite the DACL; the DACL must be
/// protected, so nothing the parent grants is inherited; and every Allow entry must name one of
/// them or the owner itself (`OW`). Deny entries narrow access and are accepted; any other kind
/// of entry is refused, as is anything this reading does not recognize. Administrators are
/// trusted because they can take ownership of any object anyway.
pub(crate) fn is_private(sddl: &str, user: &str) -> bool {
    let trusted = |sid: &str| sid == user || sid == "SY" || sid == "BA";
    let Some((owner, dacl)) = sddl
        .strip_prefix("O:")
        .and_then(|rest| rest.split_once("D:"))
    else {
        return false;
    };
    if !trusted(owner) {
        return false;
    }
    let (flags, entries) = dacl.find('(').map_or((dacl, ""), |at| dacl.split_at(at));
    if !flags.contains('P') {
        return false;
    }
    entries.split_terminator(')').all(|entry| {
        let Some(fields) = entry.strip_prefix('(').map(|entry| entry.split(';')) else {
            return false;
        };
        let fields = fields.collect::<Vec<_>>();
        match fields.as_slice() {
            ["D", _, _, _, _, _] => true,
            ["A", _, _, _, _, sid] => trusted(sid) || *sid == "OW",
            _ => false,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::is_private;

    const USER: &str = "S-1-5-21-1-2-3-1001";

    #[test]
    fn only_a_protected_dacl_for_the_user_system_and_administrators_is_private() {
        // Accepted: what a private directory carries once created, and an operator's own.
        assert!(is_private(
            "O:S-1-5-21-1-2-3-1001D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)",
            USER
        ));
        assert!(is_private(
            "O:BAD:PAI(A;OICI;FA;;;S-1-5-21-1-2-3-1001)(A;OICI;FA;;;BA)(D;;WD;;;WD)",
            USER
        ));
        assert!(is_private("O:SYD:P", USER));
        // Refused: inherited entries, another owner, another account, unknown entries.
        assert!(!is_private(
            "O:S-1-5-21-1-2-3-1001D:AI(A;OICIID;FA;;;S-1-5-21-1-2-3-1001)",
            USER
        ));
        assert!(!is_private("O:S-1-5-21-9-9-9-1002D:P(A;;FA;;;SY)", USER));
        assert!(!is_private("O:BAD:P(A;;FR;;;BU)", USER));
        assert!(!is_private("O:BAD:P(A;;FA;;;WD)", USER));
        assert!(!is_private("O:BAD:P(XA;;FA;;;SY;(x))", USER));
        assert!(!is_private("O:BAD:NO_ACCESS_CONTROL", USER));
        assert!(!is_private("D:P", USER));
    }
}
