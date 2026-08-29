//! Safe Windows process-token identity queries for private IPC authorization.

use std::{fmt::Write as _, io};

#[derive(Clone, Debug, PartialEq, Eq)]
/// Canonical binary Windows security identifier.
pub struct UserSid(Vec<u8>);

impl UserSid {
    /// Encodes this SID in the string form accepted by SDDL.
    ///
    /// # Errors
    /// Returns invalid-data when the operating system supplied a malformed SID.
    pub fn to_sddl(&self) -> io::Result<String> {
        let revision = self.0.first().copied().ok_or_else(invalid_sid)?;
        let count = self.0.get(1).copied().ok_or_else(invalid_sid)?;
        let expected = 8_usize
            .checked_add(usize::from(count).checked_mul(4).ok_or_else(invalid_sid)?)
            .ok_or_else(invalid_sid)?;
        if self.0.len() != expected {
            return Err(invalid_sid());
        }
        let authority = self.0[2..8]
            .iter()
            .fold(0_u64, |value, byte| (value << 8) | u64::from(*byte));
        let mut encoded = format!("S-{revision}-{authority}");
        for chunk in self.0[8..].chunks_exact(4) {
            let value = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            write!(&mut encoded, "-{value}").map_err(|_| invalid_sid())?;
        }
        Ok(encoded)
    }

    /// Builds a protected pipe DACL granting access only to Local System and this user.
    ///
    /// # Errors
    /// Returns invalid-data when this SID is malformed.
    pub fn private_pipe_sddl(&self) -> io::Result<String> {
        Ok(format!("D:P(A;;GA;;;SY)(A;;GA;;;{})", self.to_sddl()?))
    }
}

fn invalid_sid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Windows returned an invalid user SID",
    )
}

#[cfg(windows)]
mod windows {
    use std::{ffi::c_void, io, mem, ptr};

    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        Security::{GetLengthSid, GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser},
        System::Threading::{
            GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
        },
    };

    use super::UserSid;

    struct OwnedHandle(HANDLE);

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            // SAFETY: [Category 8 - FFI boundary] `self.0` is a non-null owned handle returned by
            // OpenProcess or OpenProcessToken and this Drop implementation runs exactly once.
            unsafe { CloseHandle(self.0) };
        }
    }

    pub fn current_user_sid() -> io::Result<UserSid> {
        // SAFETY: [Category 8 - FFI boundary] GetCurrentProcess takes no pointers and returns the
        // process pseudo-handle, which remains valid for the lifetime of this process.
        token_user_sid(unsafe { GetCurrentProcess() })
    }

    pub fn process_user_sid(process_id: u32) -> io::Result<UserSid> {
        // SAFETY: [Category 8 - FFI boundary] the access mask and inheritance flag are valid, and
        // Windows validates the untrusted process identifier before returning an owned handle.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
        if process.is_null() {
            return Err(io::Error::last_os_error());
        }
        let process = OwnedHandle(process);
        token_user_sid(process.0)
    }

    fn token_user_sid(process: HANDLE) -> io::Result<UserSid> {
        let mut token = ptr::null_mut();
        // SAFETY: [Category 8 - FFI boundary] `token` points to writable HANDLE storage and
        // `process` is either the current-process pseudo-handle or a validated owned process handle.
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = OwnedHandle(token);
        let mut required = 0_u32;
        // SAFETY: [Category 8 - FFI boundary] a null buffer with length zero is the documented size
        // query form; `required` points to initialized writable storage.
        unsafe {
            GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut required);
        }
        if required == 0 {
            return Err(io::Error::last_os_error());
        }
        let word = mem::size_of::<usize>();
        let required_usize = usize::try_from(required).map_err(|_| super::invalid_sid())?;
        let words = required_usize
            .checked_add(word - 1)
            .ok_or_else(super::invalid_sid)?
            / word;
        let mut storage = vec![0_usize; words];
        // SAFETY: [Category 8 - FFI boundary] `storage` is aligned for TOKEN_USER and spans at least
        // `required` writable bytes; Windows initializes that region on success.
        if unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                storage.as_mut_ptr().cast::<c_void>(),
                required,
                &mut required,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: [Category 8 - FFI boundary] the successful TokenUser query initialized a
        // TOKEN_USER at the aligned start of `storage` for the duration of this borrow.
        let sid = unsafe { (*storage.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        if sid.is_null() {
            return Err(super::invalid_sid());
        }
        // SAFETY: [Category 8 - FFI boundary] `sid` came from the initialized TOKEN_USER and remains
        // valid while `storage` is alive; GetLengthSid only reads that operating-system SID.
        let sid_length = unsafe { GetLengthSid(sid) };
        let sid_length = usize::try_from(sid_length).map_err(|_| super::invalid_sid())?;
        // SAFETY: [Categories 8 and 10 - FFI boundary/out-of-bounds] GetLengthSid returns the exact
        // readable byte extent of the SID embedded in the still-live token information buffer.
        let bytes = unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), sid_length) };
        Ok(UserSid(bytes.to_vec()))
    }
}

#[cfg(windows)]
pub use windows::{current_user_sid, process_user_sid};

#[cfg(test)]
mod tests {
    use super::UserSid;

    #[test]
    fn binary_sid_formats_as_canonical_sddl_identifier() {
        // Given
        let sid = UserSid(vec![1, 2, 0, 0, 0, 0, 0, 5, 32, 0, 0, 0, 33, 2, 0, 0]);

        // When
        let encoded = sid.to_sddl().expect("encode SID");

        // Then
        assert_eq!(encoded, "S-1-5-32-545");
    }

    #[test]
    fn private_pipe_dacl_names_system_and_actual_user_sid() {
        // Given
        let sid = UserSid(vec![1, 2, 0, 0, 0, 0, 0, 5, 32, 0, 0, 0, 33, 2, 0, 0]);

        // When
        let dacl = sid.private_pipe_sddl().expect("build private DACL");

        // Then
        assert_eq!(dacl, "D:P(A;;GA;;;SY)(A;;GA;;;S-1-5-32-545)");
        assert!(!dacl.contains(";;;OW)"));
    }
}
