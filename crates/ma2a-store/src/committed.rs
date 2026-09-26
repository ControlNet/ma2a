/// A value and the revision observed in the same committing Store transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Committed<T> {
    revision: u64,
    value: T,
}

impl<T> Committed<T> {
    /// Pairs an authoritative transaction result with its committed revision.
    pub const fn new(revision: u64, value: T) -> Self {
        Self { revision, value }
    }

    /// Returns the revision belonging to this value, not a later Store read.
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Borrows the authoritative value at that revision.
    pub const fn value(&self) -> &T {
        &self.value
    }

    /// Takes the authoritative value.
    pub fn into_value(self) -> T {
        self.value
    }
}
