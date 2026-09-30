//! The safe face of `tap.m`. Every call here is one C function; the unsafe
//! blocks are the whole FFI surface of the feature.

use std::ffi::CStr;
use std::os::raw::{c_char, c_double, c_int, c_uint};

unsafe extern "C" {
    fn amm_tap_available() -> c_int;
    fn amm_find_live_pid(name: *mut c_char, len: c_int) -> c_int;
    fn amm_tap_start(pid: c_int, err: *mut c_char, len: c_int) -> c_int;
    fn amm_tap_stop();
    fn amm_tap_format(rate: *mut c_double, channels: *mut c_int, buffer: *mut c_uint) -> c_int;
    fn amm_tap_drain(left: *mut f32, right: *mut f32, max_frames: c_int) -> c_int;
    fn amm_tap_frames_seen() -> u64;
    fn amm_audio_device_count() -> c_uint;
}

const ERR_LEN: usize = 512;

fn read_c_string(buf: &[c_char]) -> String {
    // SAFETY: the C side always writes a NUL inside the buffer it was given.
    unsafe { CStr::from_ptr(buf.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

/// Whether this macOS can tap another process at all (14.2 and later).
pub fn available() -> bool {
    unsafe { amm_tap_available() == 1 }
}

/// Live's process id and the name macOS shows for it, if it is running.
pub fn find_live() -> Option<(i32, String)> {
    let mut name = [0 as c_char; 128];
    let pid = unsafe { amm_find_live_pid(name.as_mut_ptr(), name.len() as c_int) };
    if pid <= 0 {
        return None;
    }
    Some((pid, read_c_string(&name)))
}

/// Start tapping one process. The error is the sentence the screen shows.
pub fn start(pid: i32) -> Result<(), String> {
    let mut err = [0 as c_char; ERR_LEN];
    let rc = unsafe { amm_tap_start(pid, err.as_mut_ptr(), ERR_LEN as c_int) };
    if rc == 0 {
        Ok(())
    } else {
        Err(read_c_string(&err))
    }
}

/// Stop and free everything. Safe to call when nothing is running.
pub fn stop() {
    unsafe { amm_tap_stop() }
}

/// Sample rate, channel count and the device's buffer size, once started.
pub fn format() -> Option<(f64, i32, u32)> {
    let (mut rate, mut channels, mut buffer) = (0.0f64, 0i32, 0u32);
    let ok = unsafe { amm_tap_format(&mut rate, &mut channels, &mut buffer) };
    (ok == 1).then_some((rate, channels, buffer))
}

/// Copy the newest audio out of the ring into `left` and `right`, returning
/// how many frames arrived. Both slices must be the same length.
pub fn drain(left: &mut [f32], right: &mut [f32]) -> usize {
    let max = left.len().min(right.len());
    if max == 0 {
        return 0;
    }
    let n = unsafe { amm_tap_drain(left.as_mut_ptr(), right.as_mut_ptr(), max as c_int) };
    n.max(0) as usize
}

/// How many audio devices the system has. The tap's private aggregate device
/// must be gone again once it stops, which is what the probe checks.
pub fn audio_device_count() -> u32 {
    unsafe { amm_audio_device_count() }
}

/// Every frame the tap has delivered since it started. Zero and staying zero
/// is how the screen knows it is being handed silence rather than audio.
pub fn frames_seen() -> u64 {
    unsafe { amm_tap_frames_seen() }
}
