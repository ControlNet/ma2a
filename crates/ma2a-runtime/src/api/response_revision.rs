use super::{CommandResult, responses::ResultKind};

impl CommandResult {
    pub(crate) const fn at_revision(mut self, revision: u64) -> Self {
        self.1 = Some(revision);
        self
    }

    pub(crate) const fn committed_revision(&self) -> Option<u64> {
        self.1
    }
}

pub(crate) const fn snapshot_revision(result: &CommandResult) -> Option<u64> {
    match &result.0 {
        ResultKind::SpaceDetails(detail) => Some(detail.revision),
        ResultKind::SnapshotStamp(stamp) => Some(stamp.revision),
        ResultKind::Snapshot(snapshot) => Some(snapshot.revision()),
        _ => None,
    }
}
