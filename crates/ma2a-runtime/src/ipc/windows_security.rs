#![cfg_attr(
    windows,
    allow(unsafe_code, reason = "audited Windows token FFI boundary")
)]

use std::{fmt::Write as _, io};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct UserSid(Vec<u8>);

impl UserSid {
    pub(super) fn to_sddl(&self) -> io::Result<String> {
        let revision = self.0.first().copied().ok_or_else(invalid_sid)?;
        let count = self.0.get(1).copied().ok_or_else(invalid_sid)?;
        let expected = 8_usize
            .checked_add(usize::from(count).checked_mul(4).ok_or_else(invalid_sid)?)
            .ok_or_else(invalid_sid)?;
        if self.0.len() != expected {
            return Err(invalid_sid());
        }
        let authority_bytes = self.0.get(2..8).ok_or_else(invalid_sid)?;
        let authority = authority_bytes
            .iter()
            .fold(0_u64, |value, byte| (value << 8) | u64::from(*byte));
        let mut encoded = format!("S-{revision}-{authority}");
        for chunk in self.0.get(8..).ok_or_else(invalid_sid)?.chunks_exact(4) {
            let bytes = <[u8; 4]>::try_from(chunk).map_err(|_| invalid_sid())?;
            let value = u32::from_le_bytes(bytes);
            write!(&mut encoded, "-{value}").map_err(|_| invalid_sid())?;
        }
        Ok(encoded)
    }

    pub(super) fn private_pipe_sddl(&self) -> io::Result<String> {
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
mod ffi {
    use std::{
        ffi::c_void,
        io, mem,
        os::windows::io::{AsHandle as _, AsRawHandle as _},
        ptr,
    };

    use interprocess::local_socket::tokio::Stream;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        Security::{
            GetLengthSid, GetTokenInformation, RevertToSelf, TOKEN_QUERY, TOKEN_USER, TokenUser,
        },
        System::{
            Pipes::ImpersonateNamedPipeClient,
            Threading::{GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken},
        },
    };

    use super::UserSid;

    struct OwnedHandle(HANDLE);

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            // SAFETY: [Category 8 - FFI boundary] `self.0` is a non-null token handle returned by
            // OpenProcessToken or OpenThreadToken, and this owner is dropped exactly once.
            unsafe { CloseHandle(self.0) };
        }
    }

    struct ImpersonationGuard {
        active: bool,
    }

    impl ImpersonationGuard {
        fn start(pipe: HANDLE) -> io::Result<Self> {
            // SAFETY: [Category 8 - FFI boundary] `pipe` is borrowed from the live accepted
            // server-side named-pipe stream and remains valid for this synchronous call.
            if unsafe { ImpersonateNamedPipeClient(pipe) } == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(Self { active: true })
            }
        }

        fn revert(mut self) -> io::Result<()> {
            // SAFETY: [Category 8 - FFI boundary] this thread successfully impersonated the pipe
            // client and has not reverted since this guard was created.
            if unsafe { RevertToSelf() } == 0 {
                Err(io::Error::last_os_error())
            } else {
                self.active = false;
                Ok(())
            }
        }
    }

    impl Drop for ImpersonationGuard {
        fn drop(&mut self) {
            if self.active {
                // SAFETY: [Category 8 - FFI boundary] an active guard means this thread is still
                // impersonating; Drop is the final retry after an explicit reversion error.
                unsafe { RevertToSelf() };
            }
        }
    }

    pub(in super::super) fn current_process_user_sid() -> io::Result<UserSid> {
        // SAFETY: [Category 8 - FFI boundary] GetCurrentProcess has no pointer arguments and returns
        // a process pseudo-handle that remains valid for this process.
        let process = unsafe { GetCurrentProcess() };
        let mut token = ptr::null_mut();
        // SAFETY: [Category 8 - FFI boundary] `token` is writable HANDLE storage and `process` is
        // the current-process pseudo-handle guaranteed by Windows.
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        token_user_sid(OwnedHandle(token))
    }

    pub(in super::super) fn impersonated_client_user_sid(stream: &Stream) -> io::Result<UserSid> {
        let pipe = match stream {
            Stream::NamedPipe(pipe) => pipe.as_handle().as_raw_handle(),
        };
        let impersonation = ImpersonationGuard::start(pipe)?;
        let sid = current_thread_user_sid();
        impersonation.revert()?;
        sid
    }

    fn current_thread_user_sid() -> io::Result<UserSid> {
        // SAFETY: [Category 8 - FFI boundary] GetCurrentThread has no pointer arguments and returns
        // the current-thread pseudo-handle, which is valid for OpenThreadToken.
        let thread = unsafe { GetCurrentThread() };
        let mut token = ptr::null_mut();
        // SAFETY: [Category 8 - FFI boundary] `token` is writable HANDLE storage; the current thread
        // is impersonating the connected pipe client and Windows validates TOKEN_QUERY access.
        if unsafe { OpenThreadToken(thread, TOKEN_QUERY, 0, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        token_user_sid(OwnedHandle(token))
    }

    fn token_user_sid(token: OwnedHandle) -> io::Result<UserSid> {
        let mut required = 0_u32;
        // SAFETY: [Category 8 - FFI boundary] a null buffer and zero length are the documented size
        // query form, and `required` is initialized writable storage.
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
        // SAFETY: [Category 8 - FFI boundary] `storage` is aligned for TOKEN_USER and contains at
        // least `required` writable bytes, which Windows initializes on success.
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
        // TOKEN_USER at the aligned start of `storage` for this borrow.
        let sid = unsafe { (*storage.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        if sid.is_null() {
            return Err(super::invalid_sid());
        }
        // SAFETY: [Category 8 - FFI boundary] `sid` points into the live initialized token buffer;
        // GetLengthSid reads only the operating-system SID header.
        let sid_length = unsafe { GetLengthSid(sid) };
        let sid_length = usize::try_from(sid_length).map_err(|_| super::invalid_sid())?;
        // SAFETY: [Categories 8 and 10 - FFI boundary/out-of-bounds] GetLengthSid returned the exact
        // readable byte extent of the SID inside the still-live token information buffer.
        let bytes = unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), sid_length) };
        Ok(UserSid(bytes.to_vec()))
    }
}

#[cfg(windows)]
pub(super) use ffi::{current_process_user_sid, impersonated_client_user_sid};

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
