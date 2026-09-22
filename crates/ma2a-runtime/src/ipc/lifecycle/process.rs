//! Which incarnation of a process identifier we are looking at.
//!
//! A process identifier is not a process. The operating system reuses it, so a
//! recorded identifier may name a completely unrelated program by the time
//! anybody reads it back. Pairing it with the moment the kernel created that
//! process gives an identity that cannot be confused with a later reuse.
//!
//! Not every platform will tell us that moment, and this deliberately answers
//! "I cannot tell" rather than guessing. Nothing destructive is ever authorized
//! by what this module returns: the daemon lock decides ownership, and a launcher
//! only ever terminates a child it still holds. This exists so diagnostics and
//! classification can distinguish a stale record from a live one.

/// A process identifier together with the incarnation of it that is running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessIncarnation {
    pid: u32,
    /// Opaque, platform-defined, and only ever compared against itself.
    started_at: Option<u64>,
}

impl ProcessIncarnation {
    /// Describes the calling process.
    #[must_use]
    pub fn current() -> Self {
        let pid = std::process::id();
        Self {
            pid,
            started_at: platform::started_at(pid),
        }
    }

    /// Rebuilds a recorded incarnation without consulting the operating system.
    #[must_use]
    pub const fn recorded(pid: u32, started_at: Option<u64>) -> Self {
        Self { pid, started_at }
    }

    /// Returns the process identifier.
    #[must_use]
    pub const fn pid(&self) -> u32 {
        self.pid
    }

    /// Returns the opaque creation stamp when the platform reports one.
    #[must_use]
    pub const fn started_at(&self) -> Option<u64> {
        self.started_at
    }

    /// Reports whether this exact incarnation is still running.
    ///
    /// `None` means the question cannot be answered here, which is not the same
    /// as "no". The platform declines for several reasons that have nothing to do
    /// with the process being gone — no permission to inspect it, no supported
    /// query, a transient failure to read — and a process that cannot be found is
    /// itself ambiguous, because an identifier is reused. Reading any of those as
    /// "dead" would turn missing evidence into a conclusion. Callers must treat
    /// `None` as unknown and fall back to an authority that does not depend on the
    /// platform, such as the daemon lock.
    #[must_use]
    pub fn is_live(&self) -> Option<bool> {
        let recorded = self.started_at?;
        // A missing reading stays missing: only two readings can disagree.
        platform::started_at(self.pid).map(|current| current == recorded)
    }
}

#[cfg(target_os = "linux")]
mod platform {
    /// Returns the kernel's creation stamp for a process, in clock ticks since boot.
    ///
    /// The second field of `stat` is the executable name in parentheses and may
    /// itself contain spaces and parentheses, so the fields are counted from the
    /// final `)` rather than by splitting the whole line.
    pub(super) fn started_at(pid: u32) -> Option<u64> {
        let status = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let after_name = status.rsplit_once(')')?.1;
        after_name.split_whitespace().nth(19)?.parse().ok()
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
mod platform {
    /// Reports no creation stamp, because this platform exposes none portably.
    pub(super) const fn started_at(_pid: u32) -> Option<u64> {
        None
    }
}

#[cfg(windows)]
#[path = "process_windows.rs"]
mod platform;

#[cfg(test)]
mod tests {
    use super::ProcessIncarnation;

    #[test]
    fn the_calling_process_is_its_own_live_incarnation() {
        // Given
        let current = ProcessIncarnation::current();

        // Then
        assert_eq!(current.pid(), std::process::id());
        assert!(matches!(current.is_live(), None | Some(true)));
    }

    #[test]
    fn a_recorded_incarnation_without_a_stamp_cannot_be_judged() {
        // Given
        let recorded = ProcessIncarnation::recorded(std::process::id(), None);

        // Then
        assert_eq!(recorded.is_live(), None);
    }

    /// Evidence that cannot be obtained stays unknown; it never becomes "dead".
    #[test]
    fn a_process_that_cannot_be_inspected_remains_unknown() {
        // Given: an identifier the operating system will not describe.
        let unknowable = ProcessIncarnation::recorded(u32::MAX, Some(1));

        // Then
        assert_eq!(unknowable.is_live(), None);
    }

    #[cfg(any(target_os = "linux", windows))]
    #[test]
    fn a_different_incarnation_of_this_identifier_is_not_live() {
        // Given
        let current = ProcessIncarnation::current();
        let stamp = current
            .started_at()
            .expect("platform reports a creation stamp");

        // When
        let impostor = ProcessIncarnation::recorded(current.pid(), Some(stamp.wrapping_add(1)));

        // Then
        assert_eq!(impostor.is_live(), Some(false));
    }
}
