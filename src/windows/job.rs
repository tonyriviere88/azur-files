//! A job object, which is the only reliable way to end a process *tree* from a program with no
//! console of its own.
//!
//! The Windows half of [`crate::console`]'s `Job`. See `Session::stop`.

use super::Job;
use std::process::Child;

impl Job {
    pub fn holding(child: &Child) -> Self {
        use std::os::windows::io::AsRawHandle as _;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };

        // SAFETY: an unnamed job object, closed by `end`.
        let Ok(job) = (unsafe { CreateJobObjectW(None, None) }) else {
            return Self(0);
        };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: the struct outlives the call and its size is its own.
        unsafe {
            let _ = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                std::mem::size_of_val(&limits) as u32,
            );
            let _ = AssignProcessToJobObject(job, HANDLE(child.as_raw_handle()));
        }
        Self(job.0 as isize)
    }

    /// End every process in the job, and release it.
    pub fn end(&mut self) {
        use windows::Win32::Foundation::{CloseHandle, HANDLE};
        use windows::Win32::System::JobObjects::TerminateJobObject;

        if self.0 == 0 {
            return;
        }
        let handle = HANDLE(self.0 as *mut std::ffi::c_void);
        self.0 = 0;
        // SAFETY: created by `holding` and cleared here, so this runs once.
        unsafe {
            let _ = TerminateJobObject(handle, 130);
            let _ = CloseHandle(handle);
        }
    }
}
