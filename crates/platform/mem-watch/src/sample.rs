//! Native footprint and host-wide pressure readings. All sizes are bytes.
#[cfg(target_os = "macos")]
pub fn process_rusage(pid: u32) -> Option<libc::rusage_info_v4> {
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v4>::uninit();
    // SAFETY: the flavor selects the size and alignment of this output buffer.
    let rc = unsafe {
        libc::proc_pid_rusage(
            pid.try_into().ok()?,
            libc::RUSAGE_INFO_V4,
            info.as_mut_ptr().cast(),
        )
    };
    // SAFETY: success means the OS initialized the requested v4 structure.
    (rc == 0).then(|| unsafe { info.assume_init() })
}

#[cfg(target_os = "macos")]
pub fn footprint(pid: u32) -> Option<u64> {
    process_rusage(pid).map(|info| info.ri_phys_footprint)
}

#[cfg(target_os = "linux")]
pub fn footprint(pid: u32) -> Option<u64> {
    // RSS is available from statm without walking the process's page tables.
    use std::sync::OnceLock;
    static PAGE: OnceLock<Option<u64>> = OnceLock::new();
    let page = (*PAGE.get_or_init(|| {
        // SAFETY: sysconf returns a scalar or -1.
        u64::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).ok()
    }))?;
    std::fs::read_to_string(format!("/proc/{pid}/statm"))
        .ok()?
        .split_whitespace()
        .nth(1)?
        .parse::<u64>()
        .ok()?
        .checked_mul(page)
}

#[cfg(windows)]
pub fn footprint(pid: u32) -> Option<u64> {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::{
            ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
            Threading::{GetCurrentProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
        },
    };
    let own = pid == std::process::id();
    // SAFETY: the current-process pseudo handle needs no close. Other handles
    // come from OpenProcess and are closed after querying process counters.
    unsafe {
        let handle = if own {
            GetCurrentProcess()
        } else {
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid)
        };
        if handle.is_null() {
            return None;
        }
        let mut counters = PROCESS_MEMORY_COUNTERS::default();
        let size = std::mem::size_of_val(&counters) as u32;
        let ok = K32GetProcessMemoryInfo(handle, &mut counters, size);
        if !own {
            CloseHandle(handle);
        }
        (ok != 0).then_some(counters.WorkingSetSize as u64)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn footprint(_: u32) -> Option<u64> {
    None
}

#[cfg(target_os = "macos")]
fn sysctl<T: Copy + Default>(name: &std::ffi::CStr) -> Option<T> {
    let mut value = T::default();
    let mut size = std::mem::size_of::<T>();
    // SAFETY: the two callers use the scalar type of their named sysctl.
    let rc = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            (&raw mut value).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    (rc == 0 && size == std::mem::size_of::<T>()).then_some(value)
}

#[cfg(target_os = "macos")]
pub(crate) fn physical_ram() -> Option<u64> {
    sysctl(c"hw.memsize")
}

/// The Mach VM pressure level is 1 at normal pressure and 2/4 at warn/critical.
#[cfg(target_os = "macos")]
pub fn pressure() -> Option<bool> {
    sysctl::<libc::c_int>(c"kern.memorystatus_vm_pressure_level")
        .and_then(|level| (level >= 1).then_some(level >= 2))
}

#[cfg(target_os = "linux")]
fn meminfo_kib(info: &str, key: &str) -> Option<u64> {
    info.lines()
        .find_map(|line| line.strip_prefix(key))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

#[cfg(target_os = "linux")]
pub(crate) fn physical_ram() -> Option<u64> {
    meminfo_kib(&std::fs::read_to_string("/proc/meminfo").ok()?, "MemTotal:")?.checked_mul(1024)
}

#[cfg(target_os = "linux")]
pub fn pressure() -> Option<bool> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    let available = meminfo_kib(&info, "MemAvailable:")?;
    let total = meminfo_kib(&info, "MemTotal:")?;
    Some(available < total / 20)
}

#[cfg(windows)]
pub(crate) fn physical_ram() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX::default();
    status.dwLength = std::mem::size_of_val(&status) as u32;
    // SAFETY: status has the size and layout requested by the API.
    (unsafe { GlobalMemoryStatusEx(&mut status) } != 0).then_some(status.ullTotalPhys)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub(crate) fn physical_ram() -> Option<u64> {
    None
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn pressure() -> Option<bool> {
    None
}
