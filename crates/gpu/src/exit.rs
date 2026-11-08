//! Leaving: no GPU work may be in flight when the process ends (issue #620).
//!
//! The devices are statics that are never dropped, and the threads that render (preview workers,
//! export, denoise, the device warm-up) are not the one that ends the process. Exiting runs the
//! driver's own teardown while those threads keep running: a render that is inside the driver at
//! that moment crashes there (seen as a SIGSEGV in the NVIDIA Vulkan driver), where neither the
//! panic hook nor `catch_unwind` can help.
//!
//! So every entry into a device goes through [`enter`], and the host ends the process with
//! [`crate::begin_shutdown`] (nothing new enters) and [`crate::wait_idle`] (what is in flight
//! finishes, up to a deadline).

#[cfg(unix)]
use std::sync::Once;
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

struct State {
    closed: bool,
    /// [`Work`] in flight, on any thread.
    busy: usize,
}

pub(crate) struct Gate {
    state: Mutex<State>,
    idle: Condvar,
}

/// GPU work in flight (until dropped).
pub(crate) struct Work<'a>(&'a Gate);

static GATE: Gate = Gate::new();
#[cfg(unix)]
static EXIT_HANDLER: Once = Once::new();

/// Start GPU work, or `None` when the process is ending: the caller then does without the GPU.
/// Work may nest (a render that creates the device).
pub(crate) fn enter() -> Option<Work<'static>> {
    GATE.enter()
}

pub(crate) fn begin_shutdown() {
    GATE.close();
}

pub(crate) fn shutting_down() -> bool {
    GATE.lock().closed
}

pub(crate) fn wait_idle(timeout: Duration) -> bool {
    GATE.wait_idle(timeout)
}

/// Install a last-resort process-exit guard after GPU initialization. Hosts should still shut down
/// their workers explicitly; this keeps test binaries and simple CLI hosts from ending while a
/// render is inside the driver.
pub(crate) fn install_process_exit_handler() {
    #[cfg(unix)]
    EXIT_HANDLER.call_once(|| {
        if !lightcraft_sysmem::register_process_exit_handler(shutdown_at_exit) {
            log::warn!("gpu: could not register process-exit shutdown guard");
        }
    });
}

#[cfg(unix)]
extern "C" fn shutdown_at_exit() {
    begin_shutdown();
    let _ = wait_idle(Duration::from_secs(120));
}

impl Gate {
    pub const fn new() -> Gate {
        Gate { state: Mutex::new(State { closed: false, busy: 0 }), idle: Condvar::new() }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn enter(&self) -> Option<Work<'_>> {
        let mut s = self.lock();
        if s.closed {
            return None;
        }
        s.busy = s.busy.saturating_add(1);
        Some(Work(self))
    }

    /// Nothing enters from now on.
    pub fn close(&self) {
        self.lock().closed = true;
    }

    /// Wait up to `timeout` until no work is in flight; `false` when some still is.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let t0 = Instant::now();
        let mut s = self.lock();
        while s.busy > 0 {
            let Some(left) = timeout.checked_sub(t0.elapsed()).filter(|d| !d.is_zero()) else { return false };
            s = self.idle.wait_timeout(s, left).unwrap_or_else(|e| e.into_inner()).0;
        }
        true
    }
}

impl Drop for Work<'_> {
    fn drop(&mut self) {
        let mut s = self.0.lock();
        s.busy = s.busy.saturating_sub(1);
        if s.busy == 0 {
            drop(s);
            self.0.idle.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_gate_lets_nothing_in_and_waits_for_what_is_inside() {
        let gate = Gate::new();
        assert!(gate.wait_idle(Duration::ZERO), "idle: no wait");
        let outer = gate.enter().unwrap();
        let inner = gate.enter().unwrap();
        gate.close();
        assert!(gate.enter().is_none(), "nothing new once closed");
        drop(inner);
        let t0 = Instant::now();
        assert!(!gate.wait_idle(Duration::from_millis(50)), "the outer work is still in flight");
        assert!(t0.elapsed() >= Duration::from_millis(50));
        std::thread::scope(|s| {
            s.spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                drop(outer);
            });
            assert!(gate.wait_idle(Duration::from_secs(30)), "woken when the work ends");
        });
        assert!(gate.enter().is_none());
    }

    #[test]
    fn work_that_panics_still_leaves() {
        let gate = Gate::new();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _work = gate.enter();
            panic!("a render panics (expected in this test)");
        }));
        assert!(r.is_err());
        assert!(gate.wait_idle(Duration::ZERO));
    }

    #[cfg(unix)]
    #[test]
    fn process_exit_guard_waits_for_in_flight_work() {
        const CHILD: &str = "LIGHTCRAFT_GPU_EXIT_GUARD_CHILD";
        const MARKER: &str = "GPU_EXIT_GUARD_WORK_FINISHED";
        if std::env::var_os(CHILD).is_some() {
            install_process_exit_handler();
            let work = enter().unwrap();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                eprintln!("{MARKER}");
                drop(work);
            });
            std::process::exit(0);
        }

        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "exit::tests::process_exit_guard_waits_for_in_flight_work", "--nocapture"])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(MARKER),
            "exit did not wait for GPU work; stderr: {}; stdout: {}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
    }
}
