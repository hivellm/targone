//! Filesystem-kind detection: the sweep protocol refuses network filesystems
//! (F-061) because Cargo skips its own locking entirely on NFS — there is no
//! safety protocol to join there.

use std::path::Path;

/// Best-effort: true when `path` lives on a network filesystem (UNC share,
/// mapped network drive, NFS/SMB/CIFS mount). Unknown ⇒ `false` for
/// reporting; the sweep layer separately refuses paths it cannot classify.
pub fn is_network_path(path: &Path) -> bool {
    imp::is_network_path(path)
}

#[cfg(windows)]
// The only unsafe in the crate: two extern syscalls with locally-built,
// NUL-terminated arguments.
#[allow(unsafe_code)]
mod imp {
    use std::path::{Component, Path, Prefix};

    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;

    /// Win32 `GetDriveType` result for a network drive (stable ABI constant).
    const DRIVE_REMOTE: u32 = 4;

    pub fn is_network_path(path: &Path) -> bool {
        let Some(Component::Prefix(prefix)) = path.components().next() else {
            // Relative path: resolve against the current dir's drive
            // (current_dir is always absolute, so this recurses at most once).
            return std::env::current_dir()
                .map(|cwd| is_network_path(&cwd))
                .unwrap_or(false);
        };
        match prefix.kind() {
            Prefix::UNC(..) | Prefix::VerbatimUNC(..) => true,
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                let root = [letter as u16, u16::from(b':'), u16::from(b'\\'), 0];
                // SAFETY: NUL-terminated wide string built just above.
                unsafe { GetDriveTypeW(root.as_ptr()) == DRIVE_REMOTE }
            }
            // DeviceNS etc.: not classifiable here — the sweep layer's
            // fail-closed policy handles it.
            _ => false,
        }
    }
}

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
mod imp {
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    // statfs f_type magics for network filesystems (linux/magic.h).
    const NFS_SUPER_MAGIC: i64 = 0x6969;
    const SMB_SUPER_MAGIC: i64 = 0x517B;
    const SMB2_MAGIC_NUMBER: i64 = 0xFE53_4D42;
    const CIFS_MAGIC_NUMBER: i64 = 0xFF53_4D42;
    const CODA_SUPER_MAGIC: i64 = 0x7375_7245;
    const AFS_SUPER_MAGIC: i64 = 0x5346_414F;
    const NCP_SUPER_MAGIC: i64 = 0x564C;
    const V9FS_MAGIC: i64 = 0x0102_1997;
    const FUSE_SUPER_MAGIC: i64 = 0x6573_5546; // sshfs & friends ride FUSE

    pub fn is_network_path(path: &Path) -> bool {
        let Ok(cstr) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
            return false;
        };
        let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statfs(cstr.as_ptr(), &mut stat) } != 0 {
            return false;
        }
        matches!(
            stat.f_type as i64,
            NFS_SUPER_MAGIC
                | SMB_SUPER_MAGIC
                | SMB2_MAGIC_NUMBER
                | CIFS_MAGIC_NUMBER
                | CODA_SUPER_MAGIC
                | AFS_SUPER_MAGIC
                | NCP_SUPER_MAGIC
                | V9FS_MAGIC
                | FUSE_SUPER_MAGIC
        )
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod imp {
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    pub fn is_network_path(path: &Path) -> bool {
        let Ok(cstr) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
            return false;
        };
        let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statfs(cstr.as_ptr(), &mut stat) } != 0 {
            return false;
        }
        let name = stat
            .f_fstypename
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as u8 as char)
            .collect::<String>();
        matches!(name.as_str(), "nfs" | "smbfs" | "afpfs" | "webdav" | "cifs")
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod imp {
    use std::path::Path;

    pub fn is_network_path(_path: &Path) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_tempdir_is_not_network() {
        let t = tempfile::tempdir().unwrap();
        assert!(!is_network_path(t.path()));
    }

    #[cfg(windows)]
    #[test]
    fn unc_paths_are_network() {
        assert!(is_network_path(Path::new(r"\\server\share\target")));
        assert!(is_network_path(Path::new(r"\\?\UNC\server\share\target")));
    }

    #[cfg(windows)]
    #[test]
    fn relative_and_device_paths() {
        // Relative: resolved via the current dir's drive (local here).
        assert!(!is_network_path(Path::new("some/relative/dir")));
        // Device namespaces are not classifiable — fail toward "not network"
        // (the sweep layer's own guards handle the rest).
        assert!(!is_network_path(Path::new(r"\\.\COM1\x")));
    }
}
