use std::{
    error::Error, fs, os::unix::fs::PermissionsExt as _, sync::mpsc, thread, time::Duration,
};

use rustix::fs::{FileType, Mode};

use super::read_owner_only_invitation_with;

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

#[test]
fn checked_regular_file_replaced_by_fifo_is_rejected_without_blocking() -> TestResult {
    // Given
    let directory =
        std::env::temp_dir().join(format!("ma2a-invite-open-boundary-{}", std::process::id()));
    let _cleanup = fs::remove_dir_all(&directory);
    fs::create_dir(&directory)?;
    let invitation = directory.join("invite.ticket");
    fs::write(&invitation, "ma2ainvite-invalid")?;
    fs::set_permissions(&invitation, fs::Permissions::from_mode(0o600))?;
    let (opened, opening) = mpsc::channel();
    let (finished, result) = mpsc::channel();
    let invitation_for_reader = invitation.clone();
    let reader = thread::spawn(move || {
        let read_result = read_owner_only_invitation_with(&invitation_for_reader, || {
            fs::remove_file(&invitation_for_reader).expect("remove checked file");
            rustix::fs::mknodat(
                rustix::fs::CWD,
                &invitation_for_reader,
                FileType::Fifo,
                Mode::from_raw_mode(0o600),
                0,
            )
            .expect("replace with FIFO");
            opened.send(()).expect("signal open boundary");
        });
        finished.send(read_result.is_err()).expect("send result");
    });
    opening.recv()?;

    // When
    let completion = result.recv_timeout(Duration::from_millis(500));

    // Then
    if completion.is_err() {
        let writer = fs::OpenOptions::new().write(true).open(&invitation)?;
        drop(writer);
    }
    reader.join().map_err(|_| "reader thread panicked")?;
    fs::remove_dir_all(&directory)?;
    assert!(completion?);
    Ok(())
}
