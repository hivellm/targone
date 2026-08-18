//! Cargo's lock protocol, joined byte-compatibly with plain `std` file locks
//! (stable since Rust 1.89 — the same `flock(2)`/`LockFileEx` primitives
//! Cargo itself uses; verified in spikes 0.1/0.2 on Windows and Linux).
//!
//! Policy: `try_lock` and skip — the sweeper never waits for a build and
//! never proceeds unlocked.

use std::fs::{File, TryLockError};
use std::io;
use std::path::Path;

/// Held for the duration of a profile sweep. Dropping releases both locks.
#[derive(Debug)]
pub struct ProfileGuard {
    /// `.cargo-build-lock`, exclusive — the real build lock (Cargo ≥1.96).
    _build: File,
    /// `.cargo-lock`, shared — interlock with pre-1.96 cargos, which took it
    /// exclusively. Shared-vs-exclusive still excludes them; shared-vs-shared
    /// does not conflict with modern cargos (they hold it shared too, but
    /// block on the build lock we hold).
    _compat: Option<File>,
}

/// `Ok(true)` = acquired, `Ok(false)` = someone holds it (skip), `Err` = the
/// filesystem refused the lock operation itself.
fn map_lock_result(r: Result<(), TryLockError>) -> io::Result<bool> {
    match r {
        Ok(()) => Ok(true),
        Err(TryLockError::WouldBlock) => Ok(false),
        Err(TryLockError::Error(e)) => Err(e),
    }
}

fn try_exclusive(f: &File) -> io::Result<bool> {
    map_lock_result(f.try_lock())
}

fn try_shared(f: &File) -> io::Result<bool> {
    map_lock_result(f.try_lock_shared())
}

/// Try to take the sweep locks on one profile directory.
/// `Ok(None)` means a build (or another sweeper) holds them — skip this
/// profile this run.
pub fn try_lock_profile(profile: &Path) -> io::Result<Option<ProfileGuard>> {
    let build = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(profile.join(".cargo-build-lock"))?;
    if !try_exclusive(&build)? {
        return Ok(None);
    }
    // Only interlock with a `.cargo-lock` that already exists: creating one
    // in a dir Cargo never made would be a write outside our mandate.
    let compat_path = profile.join(".cargo-lock");
    let compat = if compat_path.is_file() {
        let f = File::options().read(true).write(true).open(&compat_path)?;
        if !try_shared(&f)? {
            return Ok(None);
        }
        Some(f)
    } else {
        None
    };
    Ok(Some(ProfileGuard {
        _build: build,
        _compat: compat,
    }))
}

/// Probe a rustc incremental session lock: can it be held exclusively right
/// now? The handle is released immediately — under the profile build lock no
/// new rustc can start in this profile, so probe-then-delete has no live
/// racer (rustc sessions are spawned by cargo builds, which we exclude).
pub fn session_lock_free(lock_path: &Path) -> bool {
    let Ok(f) = File::options().read(true).write(true).open(lock_path) else {
        // Unreadable/absent lock file: treat as free — the file may have
        // been GC'd by rustc; deletion tolerance handles the rest.
        return true;
    };
    match f.try_lock() {
        Ok(()) => {
            let _ = f.unlock();
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_excludes_second_acquisition() {
        let t = tempfile::tempdir().unwrap();
        let guard = try_lock_profile(t.path()).unwrap();
        assert!(guard.is_some());
        // Second exclusive acquisition on the same file must fail while held.
        let again = try_lock_profile(t.path()).unwrap();
        assert!(again.is_none());
        drop(guard);
        let after = try_lock_profile(t.path()).unwrap();
        assert!(after.is_some());
    }

    #[test]
    fn compat_lock_taken_shared_when_free() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join(".cargo-lock"), b"").unwrap();
        let guard = try_lock_profile(t.path()).unwrap();
        assert!(guard.is_some());
        // While held: a pre-1.96-style exclusive acquisition must fail…
        let old_cargo = File::options()
            .read(true)
            .write(true)
            .open(t.path().join(".cargo-lock"))
            .unwrap();
        assert!(matches!(try_exclusive(&old_cargo), Ok(false)));
        // …and another shared holder coexists.
        assert!(matches!(try_shared(&old_cargo), Ok(true)));
    }

    #[test]
    fn os_lock_errors_are_surfaced_not_swallowed() {
        // A genuine I/O error must pass through — never be misread as
        // contention ("skip") or success.
        assert!(matches!(map_lock_result(Ok(())), Ok(true)));
        assert!(matches!(
            map_lock_result(Err(TryLockError::WouldBlock)),
            Ok(false)
        ));
        assert!(
            map_lock_result(Err(TryLockError::Error(io::Error::other("device says no")))).is_err()
        );
    }

    #[test]
    fn shared_compat_lock_blocks_when_held_exclusively() {
        let t = tempfile::tempdir().unwrap();
        let compat = t.path().join(".cargo-lock");
        std::fs::write(&compat, b"").unwrap();
        // Simulate a pre-1.96 cargo holding .cargo-lock exclusively.
        let old_cargo = File::options()
            .read(true)
            .write(true)
            .open(&compat)
            .unwrap();
        old_cargo.try_lock().unwrap();
        let guard = try_lock_profile(t.path()).unwrap();
        assert!(guard.is_none());
        drop(old_cargo);
    }

    #[test]
    fn missing_session_lock_is_free() {
        let t = tempfile::tempdir().unwrap();
        assert!(session_lock_free(&t.path().join("never-existed.lock")));
    }

    #[test]
    fn session_lock_probe() {
        let t = tempfile::tempdir().unwrap();
        let lock = t.path().join("s-abc-def.lock");
        std::fs::write(&lock, b"").unwrap();
        assert!(session_lock_free(&lock));
        let holder = File::options().read(true).write(true).open(&lock).unwrap();
        holder.try_lock().unwrap();
        assert!(!session_lock_free(&lock));
    }
}
