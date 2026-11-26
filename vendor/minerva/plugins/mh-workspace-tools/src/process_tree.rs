//! Process ownership outlives the shell leader and kills descendants on drop.
#[cfg(windows)]
pub struct ProcessTree(std::os::windows::io::OwnedHandle);
#[cfg(windows)]
impl ProcessTree {
    pub fn attach(child: &tokio::process::Child, name: Option<&str>) -> Result<Self, String> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle};
        use windows_sys::Win32::System::JobObjects::*;
        // The child is waiting on piped stdin: no user command runs until the
        // process is in this job, including very short commands spawning children.
        unsafe {
            let name = name.map(|name| name.encode_utf16().chain(std::iter::once(0)).collect::<Vec<_>>());
            let raw = CreateJobObjectW(std::ptr::null(), name.as_ref().map_or(std::ptr::null(), Vec::as_ptr));
            let already_exists = name.is_some()
                && std::io::Error::last_os_error().raw_os_error()
                    == Some(windows_sys::Win32::Foundation::ERROR_ALREADY_EXISTS as i32);
            if raw.is_null() {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let owned = std::os::windows::io::OwnedHandle::from_raw_handle(raw);
            if already_exists {
                return Err("Shell job name already exists".into());
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                owned.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of_val(&limits) as u32,
            ) == 0
            {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let process = child.raw_handle().ok_or("missing shell process handle")?;
            if AssignProcessToJobObject(owned.as_raw_handle(), process) == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            Ok(Self(owned))
        }
    }
    pub fn kill(&self) {
        use std::os::windows::io::AsRawHandle;
        // The owned job handle is valid for the whole call.
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0.as_raw_handle(), 1);
        }
    }

    pub fn is_empty(&mut self) -> std::io::Result<bool> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::{
            JobObjectBasicAccountingInformation, QueryInformationJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
        };
        let mut accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        // The output buffer and job handle are valid for the duration of the query.
        let ok = unsafe {
            QueryInformationJobObject(
                self.0.as_raw_handle(),
                JobObjectBasicAccountingInformation,
                (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                std::mem::size_of_val(&accounting) as u32,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(accounting.ActiveProcesses == 0)
    }
}
#[cfg(unix)]
pub struct ProcessTree(Option<libc::pid_t>);
#[cfg(unix)]
impl ProcessTree {
    pub fn attach(child: &tokio::process::Child) -> Result<Self, String> {
        let id = child.id().ok_or("missing shell process id")?;
        let group = libc::pid_t::try_from(id).map_err(|_| "invalid shell process id")?;
        if group <= 1 {
            return Err("invalid shell process group".into());
        }
        Ok(Self(Some(group)))
    }
    pub fn kill(&self) {
        // SAFETY: attach validates the positive id of the shell's own process group.
        if let Some(group) = self.0 {
            unsafe { libc::kill(-group, libc::SIGKILL) };
        }
    }

    pub fn is_empty(&mut self) -> std::io::Result<bool> {
        let Some(group) = self.0 else { return Ok(true) };
        // SAFETY: signal zero tests membership without delivering a signal.
        if unsafe { libc::kill(-group, 0) } == 0 {
            return Ok(false);
        }
        let error = std::io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::ESRCH) => {
                // Drop must not signal this number once the kernel can reuse it.
                self.0 = None;
                Ok(true)
            }
            Some(libc::EPERM) => Ok(false),
            _ => Err(error),
        }
    }
}
#[cfg(unix)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        self.kill();
    }
}
