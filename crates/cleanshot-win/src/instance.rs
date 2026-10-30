use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;
use windows::core::w;

use crate::Error;

/// Whether this process is the first Cleanshot in the user's session (C-14).
#[derive(Debug)]
pub enum Launch {
    /// This process is the one instance. Keep the value alive for the process's lifetime.
    First(SingleInstance),
    /// Another Cleanshot already runs in this session.
    AlreadyRunning,
}

/// The session-wide claim to be the one Cleanshot; dropping it releases the claim.
#[derive(Debug)]
pub struct SingleInstance(HANDLE);

impl Launch {
    /// Claims the name `Local\Cleanshot`, which lives in the session's own namespace, so
    /// another user's session never sees it.
    ///
    /// # Errors
    ///
    /// [`Error::SingleInstance`] if Windows cannot create or open the mutex.
    pub fn claim() -> Result<Self, Error> {
        // SAFETY: the name is a 'static, NUL-terminated string and no security attributes are
        // passed, so the handle is not inheritable.
        let mutex = unsafe { CreateMutexW(None, false, w!("Local\\Cleanshot")) }
            .map_err(Error::SingleInstance)?;
        // SAFETY: read straight after CreateMutexW, as its documentation requires.
        if unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
            return Ok(Self::First(SingleInstance(mutex)));
        }
        // SAFETY: the handle was opened above and is closed once, here.
        unsafe { CloseHandle(mutex) }.map_err(Error::SingleInstance)?;
        Ok(Self::AlreadyRunning)
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateMutexW and is closed once, here.
        let closed = unsafe { CloseHandle(self.0) };
        debug_assert!(closed.is_ok(), "the instance mutex closes once");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c14_a_second_claim_in_the_session_sees_the_first() {
        let first = Launch::claim().expect("the mutex is created");
        assert!(matches!(first, Launch::First(_)));
        assert!(matches!(Launch::claim(), Ok(Launch::AlreadyRunning)));
        drop(first);
        assert!(matches!(Launch::claim(), Ok(Launch::First(_))));
    }
}
