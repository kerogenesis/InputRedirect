//! Logitech G HUB, which wants the same two virtual devices we do.
//!
//! G HUB plugs its own virtual keyboard and mouse with the very product ids
//! this program asks for, and a product id can only be taken once. While its
//! agent runs, our plug is turned down with an invalid parameter - the same
//! answer the driver gives for a request it cannot read at all.
//!
//! Closing the agent once is not enough: its updater service starts it again a
//! moment later. So the service is stopped first, and a watchdog keeps looking
//! for as long as this program runs.
//!
//! Nothing of the user's is lost: profiles, macros and lighting live in the
//! application's own files and are applied again the next time it runs.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{sleep, spawn, JoinHandle};
use std::time::Duration;

use super::{process, service};

/// The processes G HUB is made of. The agent is the one that takes the virtual
/// devices; the others put it back on its feet.
const PROCESSES: [&str; 4] = [
    "lghub_agent.exe",
    "lghub.exe",
    "lghub_updater.exe",
    "lghub_system_tray.exe",
];

/// The service that starts the agent again on its own.
const UPDATER_SERVICE: &str = "LGHUBUpdaterService";

/// How long the watchdog waits between looks, and in how small a step. The step
/// is what makes stopping it feel immediate.
const WATCH_INTERVAL: Duration = Duration::from_secs(1);
const WATCH_STEP: Duration = Duration::from_millis(100);

/// Whether any part of G HUB is running.
pub fn is_running() -> bool {
    !theirs().is_empty()
}

/// Stops the updater service and closes every process of G HUB that is up.
pub fn stop() {
    // The service first: closing the agent while its updater runs buys a second.
    let _ = service::stop(UPDATER_SERVICE);

    for their_process in theirs() {
        process::terminate(their_process);
    }
}

/// Keeps G HUB closed for as long as this value is alive. Dropping it stops the
/// thread and waits for it.
pub struct Watchdog {
    watching: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Watchdog {
    pub fn start() -> Self {
        let watching = Arc::new(AtomicBool::new(true));
        let flag = Arc::clone(&watching);

        let thread = spawn(move || {
            while wait(&flag) {
                if is_running() {
                    stop();
                }
            }
        });

        Self {
            watching,
            thread: Some(thread),
        }
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.watching.store(false, Ordering::Relaxed);

        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Waits out one interval in short steps. False means the watchdog was asked to
/// stop while it waited.
fn wait(watching: &AtomicBool) -> bool {
    let mut waited = Duration::ZERO;

    while waited < WATCH_INTERVAL {
        if !watching.load(Ordering::Relaxed) {
            return false;
        }

        sleep(WATCH_STEP);
        waited += WATCH_STEP;
    }

    watching.load(Ordering::Relaxed)
}

/// The running processes that belong to G HUB.
fn theirs() -> Vec<u32> {
    process::ids_of(is_theirs)
}

/// Windows file names are case insensitive, and the whole name has to match:
/// something merely starting like theirs is somebody else's program.
fn is_theirs(name: &str) -> bool {
    PROCESSES
        .iter()
        .any(|their_name| name.eq_ignore_ascii_case(their_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_part_of_g_hub_is_recognised_whatever_case_it_is_written_in() {
        assert!(is_theirs("lghub_agent.exe"));
        assert!(is_theirs("LGHUB_AGENT.EXE"));
        assert!(is_theirs("LGHub_System_Tray.exe"));
    }

    #[test]
    fn a_program_that_only_looks_like_theirs_is_left_alone() {
        assert!(!is_theirs("lghub_agent"));
        assert!(!is_theirs("lghub_agent.exe.bak"));
        assert!(!is_theirs("my_lghub_agent.exe"));
        assert!(!is_theirs("notepad.exe"));
    }

    #[test]
    fn asking_which_of_their_processes_are_running_changes_nothing() {
        // Looking must never close anything.
        assert_eq!(theirs().len(), theirs().len());
    }

    #[test]
    fn a_watchdog_stops_and_is_waited_for_when_it_is_dropped() {
        let watchdog = Watchdog::start();
        drop(watchdog);

        // Getting here at all is the assertion: a watchdog that outlived its
        // owner would hang this test.
        let watching = AtomicBool::new(false);
        assert!(!wait(&watching));
    }
}
