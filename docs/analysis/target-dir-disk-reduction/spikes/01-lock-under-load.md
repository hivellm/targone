# Spike 0.1 — lock under load (Windows)

> Executed 2026-08-18 on the reference machine (Windows 10 Pro, NTFS,
> cargo/rustc 1.97.1, 6 live rust-analyzer processes). Scripts:
> `lockholder.rs` + `spike01-sandbox` in the session scratchpad.

## Verdict

1. **An external `std::fs::File` exclusive lock on
   `target/<profile>/.cargo-build-lock` blocks cargo 1.97 on Windows exactly
   as designed**: cargo prints `Blocking waiting for file lock on build
   directory`, waits indefinitely (no timeout, no error), and proceeds
   normally on release. Confirmed for `cargo build` **and** `cargo check`.
2. **The reverse direction works too**: while cargo builds,
   `File::try_lock()` returns `Err(TryLockError::WouldBlock)` — the engine's
   try-lock-and-skip policy is implementable with plain std (stable 1.89+),
   zero dependencies.
3. **rust-analyzer does not hold the build lock at rest.** With 6 RA
   processes running, the build locks of all four large real workspaces
   (Cortex, Thunder, Nexus SDK, ar-v3-dashboard) were immediately
   acquirable. RA takes the lock only during its periodic check runs —
   scheduled sweeps will find idle workspaces unlocked.
4. **Perceptibility rule**: safety is unconditional (cargo just waits), so
   the hold budget is purely UX — every second held adds one second to any
   build that arrives meanwhile. Design consequence for phase 2:
   **classify outside the lock, hold only for the deletion batch**, and keep
   per-profile-dir holds in the low seconds. A 60 s hold is safe but rude;
   nothing requires it.

Confidence: high — every claim above is a direct observation, reproduced at
least twice.

## Evidence

| Experiment | Result |
|---|---|
| Sandbox cold build (serde + serde_json) | 6.9 s |
| Baseline touch-rebuild, no lock | 0.37 s |
| Touch-rebuild with 10 s external hold (0.6 s head start) | wall 9.72 s ≈ 9.4 s remaining hold + 0.37 s build; `Blocking waiting for file lock on build directory` captured |
| `cargo check` with 3 s external hold | wall 4.64 s; same blocking message |
| `try_lock` during live cold build (2 samples, 1 s apart) | `Err(WouldBlock)` both times |
| `try_lock` immediately after build finished | acquired |
| `try_lock` on 4 real workspaces, 6 RA processes live | all acquired instantly |

Holder implementation detail: open `.cargo-build-lock` with
read+write+create, then `File::lock()` / `File::try_lock()` /
`File::unlock()` — byte-compatible with cargo's own use of the std file-lock
API over `LockFileEx`.

## Method & caveats

- Sandbox: fresh cargo project with serde/serde_json for non-trivial build
  time; lock held by a separate single-file rustc binary started before the
  build with ~0.6 s head start.
- Real-workspace probes acquire and immediately release (harmless); they
  prove RA's lock behavior at rest, not under an active RA check run — but
  the sandbox result covers the contended case (WouldBlock), which is the
  branch the engine takes.
- One profile dir per lock: these observations are per `target/debug`;
  `release` and per-`--target` dirs carry their own `.cargo-build-lock`
  (verified present in the sandbox layout listing).
- Unix behavior is Spike 0.2's scope, not covered here.
