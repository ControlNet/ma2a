//! Deciding from a security descriptor's SDDL text whether an object is private to one user.

/// Whether `sddl`, an owner followed by a DACL as Windows formats them, admits nobody but the
/// current user, SYSTEM, and the Administrators group.
///
/// `is_user` decides whether one SID as the text spells it, in `S-1-…` form or as a two-letter
/// alias, is the current user; on Windows the system itself parses the text, so no alias is
/// guessed from its digits. The owner must be one of those three, since an owner may rewrite the
/// DACL; the DACL must be protected, so nothing the parent grants is inherited; and every Allow
/// entry must name one of them or the owner itself (`OW`). Deny entries narrow access and are
/// accepted; any other kind of entry is refused, as is anything this reading does not recognize.
/// Administrators are trusted because they can take ownership of any object anyway.
pub(crate) fn is_private(sddl: &str, is_user: impl Fn(&str) -> bool) -> bool {
    let trusted = |sid: &str| sid == "SY" || sid == "BA" || is_user(sid);
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

    /// The user as Windows would resolve it: its full SID, or `alias` when Windows spells this
    /// account with one.
    fn user(alias: Option<&'static str>) -> impl Fn(&str) -> bool {
        move |sid| sid == USER || alias == Some(sid)
    }

    #[test]
    fn only_a_protected_dacl_for_the_user_system_and_administrators_is_private() {
        // Accepted: what a private directory carries once created, and an operator's own.
        assert!(is_private(
            "O:S-1-5-21-1-2-3-1001D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)",
            user(None)
        ));
        assert!(is_private(
            "O:BAD:PAI(A;OICI;FA;;;S-1-5-21-1-2-3-1001)(A;OICI;FA;;;BA)(D;;WD;;;WD)",
            user(None)
        ));
        assert!(is_private("O:SYD:P", user(None)));
        // Accepted: an alias the system resolves to the current user.
        assert!(is_private(
            "O:LAD:P(A;OICI;FA;;;LA)(A;OICI;FA;;;SY)",
            user(Some("LA"))
        ));
        assert!(is_private(
            "O:NSD:P(A;OICI;FA;;;NS)(A;OICI;FA;;;SY)",
            user(Some("NS"))
        ));
        // Refused: an alias that is some other account, even one with the user's RID shape.
        assert!(!is_private(
            "O:LAD:P(A;OICI;FA;;;LA)(A;OICI;FA;;;SY)",
            user(None)
        ));
        assert!(!is_private(
            "O:S-1-5-21-9-9-9-500D:P(A;;FA;;;SY)",
            user(Some("LA"))
        ));
        // Refused: inherited entries, another owner, another account, unknown entries.
        assert!(!is_private(
            "O:S-1-5-21-1-2-3-1001D:AI(A;OICIID;FA;;;S-1-5-21-1-2-3-1001)",
            user(None)
        ));
        assert!(!is_private(
            "O:S-1-5-21-9-9-9-1002D:P(A;;FA;;;SY)",
            user(None)
        ));
        assert!(!is_private("O:BAD:P(A;;FR;;;BU)", user(None)));
        assert!(!is_private("O:BAD:P(A;;FA;;;WD)", user(None)));
        assert!(!is_private("O:BAD:P(XA;;FA;;;SY;(x))", user(None)));
        assert!(!is_private("O:BAD:NO_ACCESS_CONTROL", user(None)));
        assert!(!is_private("D:P", user(None)));
    }
}
