# Spike 0.2 — Unix lock exclusion & unlink safety

> Executed 2026-08-18 in a `rust:latest` Docker container (Debian-based,
> **cargo/rustc 1.97.1**), Linux x86_64, `flock` from util-linux 2.41.
> Container FS: overlayfs; kernel 6.6.87 (WSL2 host). Scripts:
> `spike02-01-setup.sh` … `spike02-05-engine-protocol.sh` in the session
> scratchpad. Probe project: fresh `cargo new` + serde 1.0.229 (derive),
> actually used from `main.rs`.

## Verdict

**(a) Lock exclusion: GATE PASSES** (cargo 1.97.1, Linux). An external
`flock(2)` exclusive lock on `target/debug/.cargo-build-lock` blocks a
concurrent `cargo build` exactly as on Windows: cargo prints
`Blocking waiting for file lock on build directory`, waits for the full
hold with no timeout and no error, and proceeds normally on release
(wall time = remaining hold + build time, within ~0.1 s, for both 15 s
and 3 s holds). The reverse holds too: while cargo builds,
`flock -xn` on the same file is refused (exit 1) and succeeds again the
moment the build finishes — cargo holds the build-dir lock for the whole
build, so try-lock-and-skip is sound. Confidence: **high** — direct
observation, exact timings, both directions.

**(b) Unlink safety: GATE PASSES** (cargo 1.97.1, Linux). Unlinking
artifacts out of `target/` degrades to a rebuild, never to corruption:

- **b1, unlink at rest**: POSIX unlink-while-open behaved as specified — a
  held fd read the entire 1,179,138-byte rlib after `rm`. The next
  `cargo build` detected the missing output and recompiled **only** the
  deleted crate and its dependents (`serde` + `probe`, 0.46 s; the other
  5 crates untouched); the build after that was a no-op and the binary ran
  correctly. Confidence: **high**.
- **b2, unlink during a live build** (deliberate protocol violation — no
  lock taken): deleting `libserde_core-*.rlib` and `libserde-*.rlib` while
  `cargo build` ran made **that** build fail with a clean, well-shaped
  rustc error (see Evidence; exit 101 — an error, not silent wrongness),
  and the **next** `cargo build` recovered fully in 1.96 s: recompiled
  exactly the deleted crates + `probe`, then no-op, binary correct.
  No corrupted state survived. Confidence: **high** for this run;
  **medium** as a universal claim (one dependency graph, one race
  interleaving — but the engine never deletes without the lock anyway).

**Bonus — the actual engine protocol works end-to-end**: acquire
`flock -x` on `.cargo-build-lock`, delete rlibs under the lock, hold 2 s,
release. A `cargo build` launched mid-hold printed the blocking message,
waited, then rebuilt exactly `serde_core` + `serde` + `probe` (wall
3.02 s ≈ 1.5 s remaining hold + 1.5 s rebuild), succeeded, and the
follow-up build was a no-op. This is the deletion phase in miniature and
it is safe. Confidence: **high**.

**NFS (not tested, by design)**: Cargo's flock helper detects NFS and
**skips locking entirely** (known from cargo source,
`src/cargo/util/flock.rs` — `error_unsupported`/NFS carve-out). On a
network filesystem the exclusion in (a) silently does not exist, so the
engine MUST detect and refuse network filesystems rather than trust the
lock.

## Evidence

| Experiment | Result |
|---|---|
| Cold build (serde + derive, 8 crates) | 6.41 s |
| Baseline touch-rebuild, no lock | 0.14 s wall (`Finished … in 0.11s`) |
| (a) 15 s external `flock -x` hold, then touch + build | `Blocking waiting for file lock on build directory`; wall **14.80 s** ≈ 14.7 s remaining hold + 0.1 s build; exit 0 |
| (a) 3 s hold, same | same message; wall **2.80 s**; exit 0 |
| (a) reverse: `flock -xn` at t≈2 s and t≈3 s into a live cold build | **FAILED (exit 1)** both times |
| (a) reverse: `flock -xn` after `Finished` printed / after build exit | **SUCCEEDED** both times |
| (b1) `rm` rlib with fd held open | fd read all **1,179,138** bytes post-unlink; file gone from directory |
| (b1) next build | `Compiling serde` + `Compiling probe` only, **0.46 s**, exit 0; second build no-op (0.01 s); binary prints `Point { x: 1, y: 2 }` |
| (b2) delete 2 serde rlibs during live build | build failed: `error: crate `serde_core` required to be available in rlib format, but was not found in this form` → `error: could not compile `probe` (bin "probe") due to 1 previous error`; exit **101** |
| (b2) next build | `serde_core` + `serde` + `probe` recompiled, **1.96 s**, exit 0; then no-op; binary correct |
| Bonus: delete under held lock (2 s hold), concurrent build | blocking message; wall **3.02 s**; rebuilt exactly the 2 deleted crates + bin; exit 0; then no-op |

Lock-file inventory (cargo 1.97.1): `target/debug/` contains **three**
zero-byte lock files — `.cargo-build-lock`, `.cargo-artifact-lock`, and
`.cargo-lock` (legacy name still present). There is no `target/.cargo-lock`
at the top level. The build-dir lock is the one that gates builds; the
timings above confirm holding only `.cargo-build-lock` is sufficient to
exclude `cargo build`.

## Method & caveats

- All experiments inside a `rust:latest` container on its own overlayfs —
  nothing bind-mounted. `flock(2)` semantics on overlayfs upper layer are
  standard, but this is **container-on-WSL2, not bare-metal Linux**
  (kernel 6.6.87-microsoft). No known flock divergence on ext4/xfs bare
  metal, but strictly speaking that configuration is untested here.
- **macOS untested.** Cargo uses the same std/`flock` path on macOS
  (APFS supports flock), and spike 0.1 covered Windows — but (a)/(b) on
  macOS are extrapolation until someone runs this script there.
- One small dependency graph (serde tree, 8 crates); build times of
  seconds. Larger graphs give the b2 race more interleavings, but the
  failure/recovery mechanism observed (missing rlib → rustc error →
  missing-output-is-stale rebuild) is graph-size independent.
- b2 managed 2 deletions (build too fast for the planned 3–4 at 0.5 s
  spacing); both landed on crates later needed downstream, which is the
  interesting case.
- External holder was util-linux `flock(1)` (= `flock(2)` on an fd opened
  with append, never truncating the lock file). Cargo 1.96+ uses std file
  locks which are `flock(2)` on Linux — same primitive, and the observed
  mutual exclusion confirms compatibility in both directions.
- Timings measured with `date +%s%3N` around `cargo build`, launched
  ~0.3–0.5 s after the holder, so wall ≈ hold − head-start + build.
