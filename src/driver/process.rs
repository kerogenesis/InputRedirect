//! The two things this program needs to know about other processes: which of
//! them are running, and how to close one.
//!
//! Whose processes are worth closing is decided by the modules that ask.

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

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

/// Closes a process.
///
/// A process that has already gone, or one we are not allowed to close, is not
/// reported: either way it is not in the way any more.
pub fn terminate(process: u32) {
    // SAFETY: the handle is closed on every path that opened it.
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_TERMINATE, false, process) else {
            return;
        };

        let _ = TerminateProcess(handle, 0);
        let _ = CloseHandle(handle);
    }
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
        let ours = std::env::current_exe()
            .ok()
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().to_string())
            })
            .unwrap_or_default();

        assert!(!ids_of(|name| name.eq_ignore_ascii_case(&ours)).is_empty());
    }
}
