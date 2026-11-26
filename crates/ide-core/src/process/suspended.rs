use std::{
    io,
    mem::size_of,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
};
use windows_sys::Win32::{
    Foundation::{ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
        },
        Threading::{
            GetProcessIdOfThread, OpenThread, ResumeThread, THREAD_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME,
        },
    },
};

pub(super) fn resume_primary(pid: u32) -> io::Result<()> {
    // SAFETY: this takes a read-only snapshot and returns an owned handle on success.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
    let mut entry = THREADENTRY32 { dwSize: size_of::<THREADENTRY32>() as u32, ..Default::default() };
    let mut thread = None;
    // SAFETY: snapshot and entry are valid throughout the enumeration.
    let mut more = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) };
    while more != 0 {
        if entry.th32OwnerProcessID == pid && thread.replace(entry.th32ThreadID).is_some() {
            return Err(io::Error::other("Suspended process has more than one thread"));
        }
        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        more = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) };
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(ERROR_NO_MORE_FILES as i32) {
        return Err(error);
    }
    let id = thread.ok_or_else(|| io::Error::other("Suspended process has no initial thread"))?;
    // SAFETY: OpenThread returns an owned handle; ownership is rechecked before resume.
    let handle = unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION | THREAD_SUSPEND_RESUME, 0, id) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    if unsafe { GetProcessIdOfThread(handle.as_raw_handle()) } != pid {
        return Err(io::Error::other("Initial thread no longer belongs to the suspended process"));
    }
    match unsafe { ResumeThread(handle.as_raw_handle()) } {
        1 => Ok(()),
        u32::MAX => Err(io::Error::last_os_error()),
        _ => Err(io::Error::other("Unexpected initial thread suspension count")),
    }
}
