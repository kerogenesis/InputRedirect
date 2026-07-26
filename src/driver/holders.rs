//! Who is holding the driver files open.
//!
//! Removing a package from the driver store fails while its binaries are in
//! use, and Windows says so with a number. A number is not something a user can
//! act on, so Restart Manager - the same machinery an installer uses before it
//! asks for a restart - is asked who has the files open.
//!
//! Nothing here closes anything. A driver binary that is loaded is held by the
//! kernel itself, so the holder is as often `System` or a security product as
//! it is a program someone could reasonably be asked to quit. Naming it is the
//! whole purpose: with a name the user knows whether to close something or to
//! accept the restart.

use std::path::PathBuf;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
use windows::Win32::System::RestartManager::{
    RmEndSession, RmGetList, RmRegisterResources, RmStartSession, CCH_RM_SESSION_KEY,
    RM_PROCESS_INFO,
};

use super::wide;

/// How many holders are worth naming. A user reads the first few and acts on
/// them; a full register of every process that touched a file would be a wall
/// of text in a message that has to fit on one line.
const MOST_NAMES: usize = 4;

/// Names the processes holding any of `files` open.
///
/// Empty when nobody holds them - and empty as well when Restart Manager will
/// not answer, because a guess about who is in the way is worse than saying
/// nothing about it.
pub fn of(files: &[PathBuf]) -> Vec<String> {
    if files.is_empty() {
        return Vec::new();
    }

    // The wide strings have to outlive the pointers handed to Windows, so they
    // are kept in a variable of their own rather than made inside the list.
    let paths: Vec<Vec<u16>> = files
        .iter()
        .map(|file| wide(&file.display().to_string()))
        .collect();
    let pointers: Vec<PCWSTR> = paths.iter().map(|path| PCWSTR(path.as_ptr())).collect();

    let Some(session) = start_session() else {
        return Vec::new();
    };

    // SAFETY: every string in the list outlives the call, and the list is
    // described by its own length.
    let registered = unsafe { RmRegisterResources(session, Some(pointers.as_slice()), None, None) };

    let mut names = if registered == ERROR_SUCCESS {
        holders(session)
    } else {
        Vec::new()
    };

    // SAFETY: the session was started above and is not used again afterwards.
    unsafe {
        let _ = RmEndSession(session);
    }

    names.sort();
    names.dedup();
    names.truncate(MOST_NAMES);

    names
}

/// Opens a Restart Manager session, or nothing if it will not open.
fn start_session() -> Option<u32> {
    let mut session = 0u32;

    // The key is written into a buffer we own, and Restart Manager documents
    // both its length and that it has to exist before the call.
    let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];

    // SAFETY: the buffer is the documented length and outlives the call; the
    // handle is written into our own variable.
    let started = unsafe { RmStartSession(&mut session, None, PWSTR(key.as_mut_ptr())) };

    (started == ERROR_SUCCESS).then_some(session)
}

/// The holders of everything registered in the session.
fn holders(session: u32) -> Vec<String> {
    list(session).iter().map(describe).collect()
}

/// Asks for the list, once with room for a few holders and once more with room
/// for as many as Windows says there are.
///
/// The second ask is not an optimisation: when the array is too small Windows
/// fills in nothing at all and only reports how much room it wanted, so without
/// asking again there would be no list to read.
fn list(session: u32) -> Vec<RM_PROCESS_INFO> {
    let mut room = MOST_NAMES;

    for _ in 0..2 {
        let mut found = vec![RM_PROCESS_INFO::default(); room];
        let mut wanted = 0u32;
        let mut given = room as u32;
        let mut reasons = 0u32;

        // SAFETY: the array is described with the number of entries it really
        // has, and Windows fills in no more than that.
        let listed = unsafe {
            RmGetList(
                session,
                &mut wanted,
                &mut given,
                Some(found.as_mut_ptr()),
                &mut reasons,
            )
        };

        if listed == ERROR_SUCCESS {
            found.truncate(given as usize);
            return found;
        }

        if listed != ERROR_MORE_DATA || wanted as usize <= room {
            return Vec::new();
        }

        room = wanted as usize;
    }

    Vec::new()
}

/// A holder the way a user would look for it: the name they see in Task Manager
/// and the number they can find it by when two copies are running.
fn describe(holder: &RM_PROCESS_INFO) -> String {
    let name = &holder.strAppName;
    let padding = name.iter().position(|&unit| unit == 0);
    let name = String::from_utf16_lossy(&name[..padding.unwrap_or(name.len())]);
    let process = holder.Process.dwProcessId;

    if name.is_empty() {
        format!("process {process}")
    } else {
        format!("{name} (process {process})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holder(name: &str, process: u32) -> RM_PROCESS_INFO {
        let mut holder = RM_PROCESS_INFO::default();
        for (slot, unit) in holder.strAppName.iter_mut().zip(name.encode_utf16()) {
            *slot = unit;
        }
        holder.Process.dwProcessId = process;

        holder
    }

    #[test]
    fn a_holder_is_named_the_way_a_user_would_look_for_it() {
        assert_eq!(
            describe(&holder("lghub_agent.exe", 1234)),
            "lghub_agent.exe (process 1234)"
        );
    }

    /// Some holders have no name to give - a kernel-side one in particular. The
    /// number is still worth printing, and an empty pair of brackets is not.
    #[test]
    fn a_holder_without_a_name_is_still_named_by_its_number() {
        assert_eq!(describe(&holder("", 4)), "process 4");
    }

    #[test]
    fn asking_about_nothing_names_nobody() {
        assert!(of(&[]).is_empty());
    }

    #[test]
    fn nobody_holds_a_file_that_is_not_there() {
        assert!(of(&[PathBuf::from(r"Z:\no\such\folder\no_such_driver.sys")]).is_empty());
    }

    /// The file this test runs from is open by definition, so a machine where
    /// Restart Manager answers at all has to name at least one holder for it.
    /// A machine where it does not answer says nothing, which is also allowed:
    /// the point of the test is that asking is safe and does not hang.
    #[test]
    fn asking_about_a_file_that_is_open_does_not_disturb_it() {
        let Ok(ours) = std::env::current_exe() else {
            return;
        };

        let first = of(std::slice::from_ref(&ours));
        let again = of(&[ours]);

        assert_eq!(first, again);
    }
}
