//! Machine gauges read for status snapshots and 15-second heartbeats.
use crate::protocol::Load;

/// The one-minute load average per core and the memory free and in all; none where the system
/// does not say.
pub(super) async fn load() -> Option<Load> {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
    let mut average = [0f64; 1];
    // SAFETY: getloadavg writes at most one double, into this array of one.
    let read = unsafe { libc::getloadavg(average.as_mut_ptr(), 1) };
    let (free, total) = memory().await?;
    (read == 1).then_some(Load {
        cpu: average[0] / cores,
        free,
        total,
    })
}

/// Linux: what `/proc/meminfo` says is available, and the total.
#[cfg(target_os = "linux")]
async fn memory() -> Option<(u64, u64)> {
    let text = tokio::fs::read_to_string("/proc/meminfo").await.ok()?;
    let kib = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(key))?
            .split_whitespace()
            .next()?
            .parse::<u64>()
            .ok()
    };
    Some((kib("MemAvailable:")? << 10, kib("MemTotal:")? << 10))
}

/// macOS: free, file-cache and purgeable pages, which the system hands out again, and the total.
#[cfg(target_os = "macos")]
async fn memory() -> Option<(u64, u64)> {
    let sysctl = |name: &std::ffi::CStr| {
        let (mut value, mut size) = (0u64, size_of::<u64>());
        let out = (&raw mut value).cast();
        // SAFETY: the kernel writes at most `size` bytes into `value`; a 32-bit answer fills
        // its low half, which on these little-endian machines reads as the same number.
        let r =
            unsafe { libc::sysctlbyname(name.as_ptr(), out, &mut size, std::ptr::null_mut(), 0) };
        (r == 0).then_some(value)
    };
    let names = [
        c"vm.page_free_count",
        c"vm.page_pageable_external_count",
        c"vm.page_purgeable_count",
    ];
    let pages: u64 = names.into_iter().map(sysctl).sum::<Option<u64>>()?;
    Some((pages * sysctl(c"hw.pagesize")?, sysctl(c"hw.memsize")?))
}
