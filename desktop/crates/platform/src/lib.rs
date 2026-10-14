//! Narrow OS persistence boundary. Storage policy and path validation remain in artex-store.
//! Files must be sync_all'ed before install_new. Windows uses a native write-through
//! rename; Unix synchronizes the containing directory after rename. Physical storage
//! fault-injection guarantees still require qualification on each supported filesystem.
#[cfg(unix)]
use std::fs::File;
use std::{fs, io, path::Path};

#[cfg(unix)]
pub fn install_new(source: &Path, destination: &Path) -> io::Result<()> {
    if destination.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Destination exists",
        ));
    }
    fs::rename(source, destination)?;
    sync_directory(
        destination
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Missing parent"))?,
    )
}

#[cfg(windows)]
pub fn install_new(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    if source[..source.len() - 1].contains(&0) || destination[..destination.len() - 1].contains(&0)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Embedded NUL in path",
        ));
    }
    // SAFETY: both vectors are valid NUL-terminated wide paths, live for the call.
    // No REPLACE_EXISTING flag: an existing immutable destination must never be overwritten.
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
pub fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(windows)]
pub fn sync_directory(path: &Path) -> io::Result<()> {
    // Windows has no supported directory-fsync equivalent. Directory handle flush
    // is attempted and errors are classified explicitly; metadata installation uses
    // install_new's MOVEFILE_WRITE_THROUGH instead of relying on this operation.
    use std::os::windows::fs::OpenOptionsExt;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(0x02000000)
        .open(path)?;
    match file.sync_all() {
        Ok(()) => Ok(()),
        Err(e) if matches!(e.raw_os_error(), Some(1 | 5 | 6 | 50)) => Ok(()),
        Err(e) => Err(e),
    }
}

pub fn directory_flush_available() -> bool {
    cfg!(unix)
}

/// Monotonic active time since boot. Suspend is deliberately excluded; this
/// value is only meaningful for differences within the same engine process.
#[cfg(windows)]
pub fn active_clock_ms() -> io::Result<u64> {
    let mut ticks = 0u64;
    // SAFETY: an aligned writable u64 lives for the native call. Windows
    // unbiased interrupt time explicitly excludes sleep and hibernation.
    if unsafe {
        windows_sys::Win32::System::WindowsProgramming::QueryUnbiasedInterruptTime(&mut ticks)
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(ticks / 10000)
}
#[cfg(target_os = "linux")]
pub fn active_clock_ms() -> io::Result<u64> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime writes the valid timespec pointer. CLOCK_MONOTONIC
    // excludes suspend, unlike CLOCK_BOOTTIME.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let millis = (time.tv_sec as u128) * 1000 + (time.tv_nsec as u128) / 1_000_000;
    u64::try_from(millis).map_err(|_| io::Error::other("Active clock overflow"))
}
#[cfg(target_os = "macos")]
pub fn active_clock_ms() -> io::Result<u64> {
    #[repr(C)]
    struct Timebase {
        numer: u32,
        denom: u32,
    }
    unsafe extern "C" {
        fn mach_absolute_time() -> u64;
        fn mach_timebase_info(info: *mut Timebase) -> i32;
    }
    let mut timebase = Timebase { numer: 0, denom: 0 };
    // SAFETY: native output pointer is valid. mach_absolute_time's uptime clock
    // excludes suspend; mach_continuous_time is intentionally not used.
    if unsafe { mach_timebase_info(&mut timebase) } != 0 || timebase.denom == 0 {
        return Err(io::Error::other("Mach timebase unavailable"));
    }
    let ticks = unsafe { mach_absolute_time() };
    let millis = (ticks as u128) * (timebase.numer as u128) / (timebase.denom as u128) / 1_000_000;
    u64::try_from(millis).map_err(|_| io::Error::other("Active clock overflow"))
}
#[cfg(test)]
mod tests {
    #[test]
    fn active_clock_is_monotonic() {
        let before = super::active_clock_ms().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let after = super::active_clock_ms().unwrap();
        assert!(after >= before + 10);
    }
}
