use super::{CommandResult, responses::ResultKind};

pub(crate) const fn snapshot_revision(result: &CommandResult) -> Option<u64> {
    match &result.0 {
        ResultKind::SpaceDetails(detail) => Some(detail.revision),
        ResultKind::SnapshotStamp(stamp) => Some(stamp.revision),
        ResultKind::Snapshot(snapshot) => Some(snapshot.revision()),
        _ => None,
    }
}
