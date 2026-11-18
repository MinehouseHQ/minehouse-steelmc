//! Vanilla `MinecraftServer.warnOnLowDiskSpace` and
//! `LevelStorageAccess.checkForLowDiskSpace`.

#[cfg(unix)]
use std::ffi::CString;
#[cfg(unix)]
use std::mem::MaybeUninit;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;

use steel_protocol::packets::game::CLowDiskSpaceWarning;

use super::Server;

/// Vanilla warns below 64 MiB of usable space.
const LOW_DISK_SPACE_BYTES: u64 = 64 * 1024 * 1024;

impl Server {
    /// Vanilla `warnOnLowDiskSpace`, run after every save.
    pub(crate) fn warn_on_low_disk_space(&self) {
        if usable_space(&self.save_root) < LOW_DISK_SPACE_BYTES {
            self.send_low_disk_space_warning();
        }
    }

    /// Vanilla `DedicatedServer.sendLowDiskSpaceWarning`: logs, then tells every admin.
    pub(super) fn send_low_disk_space_warning(&self) {
        log::warn!("Low disk space! Might not be able to save the world.");
        for player in self.get_players() {
            if self.is_operator(player.gameprofile.id()) {
                player.send_packet(CLowDiskSpaceWarning);
            }
        }
    }
}

/// Vanilla `LevelStorageAccess.estimateDiskSpace`: the usable bytes of the save
/// directory's file system, or `u64::MAX` when it cannot be read.
#[cfg(unix)]
pub(super) fn usable_space(path: &Path) -> u64 {
    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
        return u64::MAX;
    };
    let mut stats = MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is NUL-terminated and `stats` is valid for writes of one `statvfs`.
    if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return u64::MAX;
    }
    // SAFETY: `statvfs` returned 0, so it filled `stats`.
    let stats = unsafe { stats.assume_init() };
    let bytes = u128::from(stats.f_bavail) * u128::from(stats.f_frsize);
    u64::try_from(bytes).unwrap_or(u64::MAX)
}

/// Vanilla `LevelStorageAccess.estimateDiskSpace` without a file system query.
#[cfg(not(unix))]
pub(super) fn usable_space(_path: &Path) -> u64 {
    u64::MAX
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::path::Path;

    use super::usable_space;

    #[test]
    fn usable_space_reads_the_file_system_or_gives_up() {
        let temp = usable_space(&env::temp_dir());
        assert!(
            temp > 0 && temp < u64::MAX,
            "the temp dir's file system is readable"
        );
        assert_eq!(usable_space(Path::new("/no/such/steel/save")), u64::MAX);
    }
}
