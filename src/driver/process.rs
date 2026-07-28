//! The two things this program needs to know about other processes: which of
//! them are running, and how to close one.
//!
//! Whose processes are worth closing is decided by the modules that ask.

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
};

/// Long enough for any path Windows will hand back, including the long ones.
const LONGEST_PATH: usize = 1024;

/// The ids of the running processes whose file name `wanted` accepts.
///
/// Looking changes nothing, which is worth saying because the caller usually
/// looks in order to close something afterwards.
pub fn ids_of(wanted: impl Fn(&str) -> bool) -> Vec<u32> {
    let mut found = Vec::new();

    // SAFETY: the snapshot is closed on every path out of here, and the entry
    // is given its real size before Windows is asked to fill it in.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return found;
        };

        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };

        let mut listed = Process32FirstW(snapshot, &mut entry).is_ok();
        while listed {
            if wanted(&name_of(&entry)) {
                found.push(entry.th32ProcessID);
            }

            listed = Process32NextW(snapshot, &mut entry).is_ok();
        }

        let _ = CloseHandle(snapshot);
    }

    found
}

/// Closes a process, but only if its image name is still one `wanted` accepts.
///
/// The id alone is not evidence of what a process is. Windows hands ids out
/// again as soon as they are free, and every caller here listed the process
/// some time before deciding to close it - the watchdog does so about once a
/// second for as long as the program runs. Between the listing and the call the
/// process can exit and its id be given to something else, which this program
/// would then close while running as an administrator.
///
/// The name is therefore read back from the very handle that is about to be
/// used, not from the id: the handle keeps the process it named, so nothing can
/// change between the question and the answer.
///
/// A process that has already gone, or one we are not allowed to close, is not
/// reported: either way it is not in the way any more.
pub fn terminate(process: u32, wanted: impl Fn(&str) -> bool) {
    // SAFETY: the handle is closed on every path that opened it.
    unsafe {
        let Ok(handle) = OpenProcess(
            PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            process,
        ) else {
            return;
        };

        if image_name(handle).is_some_and(|name| wanted(&name)) {
            let _ = TerminateProcess(handle, 0);
        }

        let _ = CloseHandle(handle);
    }
}

/// The file name of the running image behind a handle, without its directory.
///
/// `None` when Windows will not say - a process that is already exiting, or one
/// this program may not ask about. Nothing is closed on an answer like that.
fn image_name(handle: HANDLE) -> Option<String> {
    let mut buffer = [0u16; LONGEST_PATH];
    let mut written = buffer.len() as u32;

    // SAFETY: the buffer is described with its real length, and Windows writes
    // back how much of it was used.
    unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut written,
        )
        .ok()?;
    }

    let path = String::from_utf16_lossy(&buffer[..written as usize]);
    let name = path.rsplit(['\\', '/']).next()?;

    Some(name.to_owned())
}

/// The file name Windows reports for a process, without the padding that
/// follows it in the fixed-size field.
fn name_of(entry: &PROCESSENTRY32W) -> String {
    let name = &entry.szExeFile;
    let padding = name.iter().position(|&unit| unit == 0);

    String::from_utf16_lossy(&name[..padding.unwrap_or(name.len())])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> PROCESSENTRY32W {
        let mut entry = PROCESSENTRY32W::default();
        for (slot, unit) in entry.szExeFile.iter_mut().zip(name.encode_utf16()) {
            *slot = unit;
        }

        entry
    }

    #[test]
    fn a_name_stops_where_the_padding_starts() {
        assert_eq!(name_of(&entry("lghub_agent.exe")), "lghub_agent.exe");
        assert_eq!(name_of(&entry("")), "");
    }

    #[test]
    fn a_name_no_process_has_matches_nothing() {
        assert!(ids_of(|name| name == "no_such_process_of_ours.exe").is_empty());
    }

    /// Every machine runs at least the process asking the question.
    #[test]
    fn the_listing_finds_the_program_that_is_asking() {
        assert!(!ids_of(|name| name.eq_ignore_ascii_case(&ours())).is_empty());
    }

    /// The process this test runs in is the one case where the outcome of a
    /// refused termination can be observed: reaching the next line is it.
    #[test]
    fn a_process_whose_name_is_not_wanted_is_left_running() {
        terminate(std::process::id(), |name| {
            name == "no_such_process_of_ours.exe"
        });
    }

    /// Whatever the predicate says, an id nothing is running under closes
    /// nothing. Id 0 belongs to the idle process and cannot be opened.
    #[test]
    fn an_id_that_names_nothing_closes_nothing() {
        terminate(0, |_| true);
    }

    /// The check has to see the same name the listing did, or the callers would
    /// pass a predicate that never matches and close nothing at all.
    #[test]
    fn the_name_read_from_a_handle_is_the_one_the_listing_reports() {
        // SAFETY: a pseudo handle to this process; nothing is opened or closed.
        let ourselves =
            unsafe { windows::Win32::System::Threading::GetCurrentProcess() };

        assert_eq!(
            image_name(ourselves).map(|name| name.to_lowercase()),
            Some(ours().to_lowercase())
        );
    }

    /// The file name of the running test binary.
    fn ours() -> String {
        std::env::current_exe()
            .ok()
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().to_string())
            })
            .unwrap_or_default()
    }
}
