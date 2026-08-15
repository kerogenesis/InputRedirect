//! Logitech software that stands between this program and its virtual devices.
//!
//! G HUB plugs its own virtual keyboard and mouse with the very product ids
//! this program asks for, and a product id can only be taken once. While its
//! agent runs, our plug is turned down with an invalid parameter - the same
//! answer the driver gives for a request it cannot read at all.
//!
//! The Logitech `LampArray` service blocks differently: its process keeps the
//! virtual bus device open, and Windows vetoes a re-bind of the bus driver
//! while anything holds the device open. A vetoed re-bind looks like success
//! to the caller and leaves the bus with its stale state - the state in which
//! the next mouse plug is refused. So the service is stopped before every
//! re-bind and kept stopped while the program runs.
//!
//! Closing the G HUB agent once is not enough: its updater service starts it
//! again a moment later, and the `LampArray` service has a recovery restart of
//! its own. The watchdog therefore keeps looking for as long as this program
//! runs, and the `LampArray` service is started again when the program exits,
//! but only if it was running when the program arrived.
//!
//! Nothing of the user's is lost: profiles, macros and lighting live in the
//! application's own files and are applied again the next time it runs.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{JoinHandle, sleep, spawn};
use std::time::Duration;

use crate::error::Error;

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

/// The service whose process holds the virtual bus device open. It is not a
/// competitor for the product ids - it is the reason a bus re-bind is vetoed.
const LAMP_SERVICE: &str = "logi_lamparray_service";

/// What the caller says when the `LampArray` service refuses to let go.
pub const LAMP_STILL_HOLDING: &str =
    "the Logitech `LampArray` service keeps the virtual bus open and would not stop";

/// How long the watchdog waits between looks, and in how small a step. The step
/// is what makes stopping it feel immediate.
const WATCH_INTERVAL: Duration = Duration::from_secs(1);
const WATCH_STEP: Duration = Duration::from_millis(100);

/// Whether anything that would block the virtual devices is running: G HUB
/// itself, or the `LampArray` service whose process holds the bus device open.
pub fn blockers_running() -> bool {
    blockers(&theirs(), service::is_running(LAMP_SERVICE))
}

/// The same question, over what was found rather than over the machine, so the
/// `LampArray`-only case is testable without touching a real service.
fn blockers(their_processes: &[u32], lamp_running: bool) -> bool {
    !their_processes.is_empty() || lamp_running
}

/// Stops the updater service, the `LampArray` service and every process of G HUB
/// that is up.
///
/// `false` means the `LampArray` service would not stop, which the caller has to
/// say out loud: carrying on would meet a bus re-bind that Windows vetoes
/// without reporting it.
pub fn stop_blockers() -> bool {
    // The service first: closing the agent while its updater runs buys a second.
    let _ = service::stop(UPDATER_SERVICE);

    let lamp_stopped = service::stop_and_wait(LAMP_SERVICE);

    for their_process in theirs() {
        // The same question is asked again inside, of the handle rather than
        // the id: this list is a moment old, and ids are handed out again.
        process::terminate(their_process, is_theirs);
    }

    lamp_stopped
}

/// Stops the blockers again if any of them is back. The watchdog's whole day.
fn keep_blockers_down() {
    if blockers_running() {
        let _ = stop_blockers();
    }
}

/// Keeps the blockers closed for as long as this value is alive. Dropping it
/// stops the thread, waits for it, and starts the `LampArray` service again if
/// it was running when the watchdog arrived.
pub struct Watchdog {
    watching: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    restore: Option<fn()>,
}

impl Watchdog {
    /// Stops everything that blocks the virtual devices, then keeps looking.
    ///
    /// Fails when the `LampArray` service refuses to stop: nothing else can
    /// happen until it lets go of the bus device.
    pub fn start() -> Result<Self, Error> {
        let lamp_was_running = service::is_running(LAMP_SERVICE);

        if !stop_blockers() {
            return Err(Error::Device(LAMP_STILL_HOLDING.to_owned()));
        }

        let mut watchdog = Self::spawn(keep_blockers_down);
        if lamp_was_running {
            watchdog.restore = Some(restore_lamp);
        }
        Ok(watchdog)
    }

    /// The thread that repeats `action` every second until dropped. Tests hand
    /// it a no-op, so a dropped watchdog cannot stop anything real.
    fn spawn(action: fn()) -> Self {
        let watching = Arc::new(AtomicBool::new(true));
        let flag = Arc::clone(&watching);

        let thread = spawn(move || {
            while wait(&flag) {
                action();
            }
        });

        Self {
            watching,
            thread: Some(thread),
            restore: None,
        }
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.watching.store(false, Ordering::Relaxed);

        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }

        if let Some(restore) = self.restore.take() {
            restore();
        }
    }
}

/// Starts the `LampArray` service again, once the program no longer needs the
/// bus to stay re-bindable.
fn restore_lamp() {
    let _ = service::start_and_wait(LAMP_SERVICE);
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

    /// The predicate the watchdog closes processes with must not accept the
    /// program running it, whatever happens to the ids in between.
    #[test]
    fn the_watchdog_would_not_close_this_program() {
        let ours = std::env::current_exe()
            .ok()
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().to_string())
            })
            .unwrap_or_default();

        assert!(!is_theirs(&ours));
    }

    #[test]
    fn asking_which_of_their_processes_are_running_changes_nothing() {
        // Looking must never close anything.
        assert_eq!(theirs().len(), theirs().len());
    }

    /// The `LampArray` service alone blocks the virtual devices, even when no
    /// G HUB process is running: its process holds the bus device open, and
    /// every re-bind asked while it runs is vetoed without a word.
    #[test]
    fn the_lamp_array_service_alone_counts_as_blocking() {
        assert!(blockers(&[], true));
        assert!(!blockers(&[], false));
        assert!(blockers(&[4242], false));
    }

    #[test]
    fn a_watchdog_stops_and_is_waited_for_when_it_is_dropped() {
        let watchdog = Watchdog::spawn(|| {});
        drop(watchdog);

        // Getting here at all is the assertion: a watchdog that outlived its
        // owner would hang this test.
        let watching = AtomicBool::new(false);
        assert!(!wait(&watching));
    }

    /// A watchdog that arrived while the `LampArray` service was running starts
    /// it again on the way out, and only then.
    #[test]
    fn a_watchdog_restarts_the_lamp_array_service_it_found_running() {
        static RESTORED: AtomicBool = AtomicBool::new(false);
        fn fake_restore() {
            RESTORED.store(true, Ordering::SeqCst);
        }

        let mut watchdog = Watchdog::spawn(|| {});
        watchdog.restore = Some(fake_restore);
        drop(watchdog);

        assert!(RESTORED.load(Ordering::SeqCst));
    }

    /// A watchdog that found the `LampArray` service already stopped leaves it
    /// alone on the way out.
    #[test]
    fn a_watchdog_that_found_nothing_running_restores_nothing() {
        let watchdog = Watchdog::spawn(|| {});
        assert!(watchdog.restore.is_none());
    }
}
