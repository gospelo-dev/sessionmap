//! Per-process memory accounting.
//!
//! RSS is the wrong number to show on macOS: the kernel compresses the pages of
//! a process nobody touches, so an idle session's RSS falls while the memory is
//! still held. That is backwards for this tool — the longer a session sits, the
//! smaller it looks. Apple's own tools report `phys_footprint` instead (the
//! "Memory" column in Activity Monitor), which counts compressed and swapped
//! pages as well, so that is what we use, falling back to RSS when it is not
//! available (other user's process, process already gone).
//!
//! Linux has no equivalent of the compressor in the default configuration, and
//! on Windows `sysinfo` already reports the private working set, so both keep
//! using the value `sysinfo` gives us.

use sysinfo::Process;

#[cfg(target_os = "macos")]
mod macos {
    /// `struct rusage_info_v0` from `<libproc.h>`. The footprint is present in
    /// this oldest flavor already, so there is no reason to ask for a newer one.
    #[repr(C)]
    #[derive(Default)]
    struct RusageInfoV0 {
        ri_uuid: [u8; 16],
        ri_user_time: u64,
        ri_system_time: u64,
        ri_pkg_idle_wkups: u64,
        ri_interrupt_wkups: u64,
        ri_pageins: u64,
        ri_wired_size: u64,
        ri_resident_size: u64,
        ri_phys_footprint: u64,
        ri_proc_start_abstime: u64,
        ri_proc_exit_abstime: u64,
    }

    const RUSAGE_INFO_V0: i32 = 0;

    unsafe extern "C" {
        fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut RusageInfoV0) -> i32;
    }

    pub fn phys_footprint(pid: u32) -> Option<u64> {
        let mut info = RusageInfoV0::default();
        // SAFETY: the buffer layout matches the flavor we ask for, and libproc
        // writes at most that many bytes into it.
        let rc = unsafe { proc_pid_rusage(pid as i32, RUSAGE_INFO_V0, &mut info) };
        (rc == 0 && info.ri_phys_footprint > 0).then_some(info.ri_phys_footprint)
    }
}

/// Memory held by a single process, in bytes.
pub fn proc_mem(p: &Process) -> u64 {
    #[cfg(target_os = "macos")]
    if let Some(footprint) = macos::phys_footprint(p.pid().as_u32()) {
        return footprint;
    }
    p.memory()
}

/// What `proc_mem` measures on this platform, for the header line.
pub const METRIC: &str = if cfg!(target_os = "macos") { "footprint" } else { "RSS" };

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(target_os = "macos")]
    fn footprint_of_this_process_is_plausible() {
        let me = std::process::id();
        let footprint = super::macos::phys_footprint(me).expect("own process has a footprint");
        assert!(footprint > 1 << 20, "footprint {footprint} looks too small");
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn footprint_of_a_dead_pid_is_none() {
        // pid 0 is the kernel task: never ours to inspect, so this must not panic
        // and must fall back cleanly.
        assert_eq!(super::macos::phys_footprint(0), None);
    }
}
