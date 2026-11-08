//! Serialize UI tests that create or initialize native GPU devices.
//!
//! The NVIDIA driver on the test host crashes while tearing down multiple wgpu devices used by
//! concurrent tests. Rendering code remains concurrent; only tests that own or explicitly start a
//! device take this process-local lock.

static DEVICE_TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) fn device_turn() -> std::sync::MutexGuard<'static, ()> {
    DEVICE_TURN.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
