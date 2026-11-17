//! Lower OS scheduling priority for background chunk generation threads.
//!
//! Generation and the game tick share the same cores. With many players exploring, generation
//! keeps every core busy and the tick workers wait for a time slice, so the whole tick slows
//! down. Generation threads ask the OS to run them behind the tick: they still use every core
//! the tick leaves idle. `STEEL_GENERATION_PRIORITY=normal` keeps the default priority.

use std::{env, io};

/// Nice value for generation threads on Linux (0 is normal, 19 is lowest).
#[cfg(target_os = "linux")]
const GENERATION_NICE: libc::c_int = 10;

/// Whether generation threads should drop their scheduling priority.
pub(crate) fn lower_generation_priority_enabled() -> bool {
    !env::var("STEEL_GENERATION_PRIORITY").is_ok_and(|value| value.eq_ignore_ascii_case("normal"))
}

/// Lowers the calling thread's scheduling priority, logging when the OS refuses.
pub(crate) fn lower_current_thread_priority() {
    if let Err(error) = lower_priority() {
        log::debug!("Could not lower generation thread priority: {error}");
    }
}

#[cfg(target_os = "linux")]
fn lower_priority() -> io::Result<()> {
    // SAFETY: `gettid` takes no arguments and only returns the calling thread's id.
    let thread_id = unsafe { libc::syscall(libc::SYS_gettid) };
    let thread_id = libc::id_t::try_from(thread_id).map_err(io::Error::other)?;
    // SAFETY: on Linux `PRIO_PROCESS` with a thread id changes only that thread's nice value.
    let result = unsafe { libc::setpriority(libc::PRIO_PROCESS, thread_id, GENERATION_NICE) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "macos")]
fn lower_priority() -> io::Result<()> {
    // SAFETY: sets the QoS class of the calling thread only; both arguments are plain values.
    let result =
        unsafe { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(result))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "matches the platform implementations"
)]
const fn lower_priority() -> io::Result<()> {
    Ok(())
}
