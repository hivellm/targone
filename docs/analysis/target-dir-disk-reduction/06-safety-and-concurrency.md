# 06 — Safety: concurrency, Windows, and the failure modes that matter

A tool with delete authority over 300 GB of build state has exactly one
unacceptable failure: corrupting a build that is in progress. This file
establishes what protection exists, what does not, and what the implementation
must therefore do itself.

---

## F-050 — Cargo's locks are exclusive, live in the profile directory, and are externally detectable

Cargo 1.97 places three lock files in `target/<profile>/`:

```
target/debug/.cargo-lock
target/debug/.cargo-build-lock
target/debug/.cargo-artifact-lock
```

All are zero bytes. There is **no** `target/.cargo-lock` at the root.

Probe: a 25-second build was started in the background; an external process then
attempted to open each lock. Control run with no build in progress:

| lock | during a build (shared read) | during a build (exclusive) | no build running |
|---|---|---|---|
| `.cargo-lock` | **DENIED** | **DENIED** | FREE |
| `.cargo-build-lock` | **DENIED** | **DENIED** | FREE |
| `.cargo-artifact-lock` | **DENIED** | **DENIED** | FREE |

**Impact.** A reliable, zero-cost build-in-progress detector exists on Windows:
attempt an exclusive open of `target/<profile>/.cargo-lock`; if it is denied, a
build owns the directory and the cleaner must skip it. This must be the very
first check on every profile directory, and it must be re-checked (see F-054).

Cross-platform caveat: on Unix, Cargo uses `flock`, which is **advisory** — an
external process can read and delete freely unless it also takes the lock. The
detector must therefore be `flock(LOCK_EX|LOCK_NB)` on Unix, not an open()
attempt, and holding it is what actually excludes Cargo. This is a genuine
implementation fork, not a portability detail to paper over. The `cargo-util`
crate exposes Cargo's own locking; using it is preferable to reimplementing.

**Confidence: high** on Windows (measured, with control); **medium** on Unix
(inferred from Cargo's use of `flock`, not measured here).

---

## F-051 — The locks protect nothing on their own: artifacts were deleted mid-build

During the same in-progress build, an external process deleted
`libtargone_probe-b649025af5a2b45b.rlib` from `target/debug/deps/`. The
deletion succeeded, and the build went on to complete normally.

**Impact.** Two conclusions, and the second is the important one:

1. Cargo's locks are a cooperation protocol between well-behaved tools. The
   filesystem enforces nothing. A cleaner that skips the lock check will corrupt
   builds, and will do so *intermittently* — the deletion above happened to be
   harmless because Cargo was already past that file. The same deletion a second
   earlier would have produced a link failure or a corrupt artifact.
2. Because the damage is timing-dependent, it will not show up in testing. It
   will show up as a rare, unreproducible build failure that the user attributes
   to Cargo, months after adopting the tool. **Trust in the tool is the asset
   being protected, and it is destroyed by exactly one such incident.** This
   argues for a conservative default and a loud, auditable log of every deletion.

**Confidence: high** (direct experiment).

---

## F-052 — Six rust-analyzer processes are running right now; concurrent builds are the normal state

```
6 × rust-analyzer            (started 17/08 01:41, 17/08 07:00, 18/08 03:55)
2 × rust-analyzer-proc-macro-srv
```

And every large project has a `target/flycheck0/` directory — but it contains
only `stdout` and `stderr`, meaning rust-analyzer's `cargo check` is **writing
into `target/debug/` itself**, not into a private target directory.

Cortex additionally has `target/llvm-cov-target/` (from `cargo-llvm-cov`) and
every project has `target/tmp/`.

**Impact.** The dangerous window is not "while the user runs `cargo build`". An
IDE-driven `cargo check` can start at any moment — on file save, on focus, on a
timer — in any of six open workspaces, and it writes to the same `debug/`
directory a cleaner would be pruning. This raises the concurrency requirement
from "handle the occasional overlap" to "assume a build may begin at any instant
and design for it". It also means the check-then-delete window (F-054) must be
kept very short.

Note also that `flycheck0`, `llvm-cov-target` and `tmp` are additional
target-like subtrees: the scanner must enumerate profile directories rather than
assuming `debug/` and `release/`.

**Confidence: high** (direct observation).

---

## F-053 — Windows blocks deleting *and renaming* open files, and fails recursive deletes halfway

Measured against files held open with the sharing modes a compiler or linker
would use:

| scenario | result |
|---|---|
| delete file opened with `FileShare.Read` | **BLOCKED** — "being used by another process" |
| delete file opened with `FileShare.ReadWrite \| Delete` | succeeds; name vanishes immediately |
| delete file that is memory-mapped | **BLOCKED** |
| recursive delete of a directory containing an open file | **BLOCKED**, after partially deleting siblings |
| **rename** a file opened with `FileShare.Read` | **BLOCKED** |

**Impact.** Three concrete design consequences:

1. **The Unix "move it aside, delete later" trick does not work on Windows.**
   Renaming an open file is refused just as deletion is. Any design that assumes
   it can atomically retire a file by renaming it must have a Windows fallback:
   attempt, catch the sharing violation, skip, and retry on a later pass.
2. **Recursive directory deletion is not atomic and fails dirty.** Deleting an
   `incremental/<crate>-<hash>/` directory can remove half its contents and then
   hit an open file. A half-deleted incremental directory is worse than an intact
   one — rustc may find a corrupt session. Deletions must be per-file with
   per-file error tolerance, and a directory must only be removed once it is
   verifiably empty.
3. **Sharing violations are the *expected* case, not an error.** They are the
   filesystem correctly protecting a live build. The cleaner must treat
   `ERROR_SHARING_VIOLATION` as "skip, try next time", never as a failure to
   report or a reason to retry aggressively.

On Unix the semantics invert: `unlink()` of an open file always succeeds and the
data survives until the last descriptor closes. That is *safer* for the running
build but means freed bytes do not appear until the compiler exits — so
post-sweep space reporting must not assume immediate reclamation.

**Confidence: high** (direct experiment on Windows); **medium** for the Unix
description (standard POSIX semantics, not measured here).

---

## F-054 — The check-then-delete window is the core hazard; the protocol must close it

Combining F-050, F-051 and F-052: a build can start between the lock check and
the deletion. The required protocol:

1. Verify the directory is a Cargo target dir (`CACHEDIR.TAG` present, F-017).
2. **Acquire** the profile's Cargo lock — do not merely test it. Holding it is
   what makes the operation safe, because Cargo will then wait rather than start.
3. Enumerate and decide **while holding the lock** — metadata only, never opening
   file contents (F-047).
4. Delete while holding the lock, per file, tolerating sharing violations.
5. Release.

If the sweep would hold the lock too long to be polite, it should instead
process one profile directory at a time and release between them, so an
interactive build waits seconds rather than minutes.

**Impact.** "Check the lock, then delete" is insufficient and must be stated as
such, because it is the obvious implementation and it is subtly wrong. Note the
tension with F-052: holding the lock blocks rust-analyzer's `cargo check`, which
the user experiences as the IDE hanging. The sweep must therefore be both
lock-holding *and* short — which argues for incremental sweeps (one profile dir,
bounded file count) over one long pass, and for scheduling sweeps when the
machine is idle.

**Confidence: high** for the hazard; **medium** for the specific protocol
(designed here, not yet validated under load — see `08-execution-plan.md`).

---

## F-055 — `CACHEDIR.TAG` alone misses 57% of the bytes; the discriminator must be composite

Scanning two source roots for directories named `target` returned **48** hits, of
which **30 were false positives** — `llvm/lib/Target`, `lldb/source/Target`,
`mlir/include/mlir/Target`, `clang/include/clang/Basic/Target` and similar,
inside vendored LLVM and GCC trees under `E:\HiveLLM\Tml`. A name-based scanner
pointed at this machine would have deleted parts of the LLVM and GCC source
trees, so a marker-based precondition is mandatory.

The obvious marker is not sufficient. Measured across the 18 real target
directories:

| discriminator | catches |
|---|---:|
| `CACHEDIR.TAG` at the target root | **14 / 18** |
| `.rustc_info.json` at the target root | 15 / 18 |
| a profile subdirectory containing `.fingerprint/` | 15 / 18 |
| **any of the three** | **15 / 18** |

The three missed by every test are the empty husks (0 files) — correctly
uninteresting. All five sampled LLVM/GCC directories have **none** of the three.

The critical case:

```
root   debug  rel    rustc_info  path
False  False  False  True        E:\HiveLLM\Cortex\target        <-- 172 GB, 57% of all bytes
True   False  False  True        E:\HiveLLM\Thunder\rust\target
True   False  False  True        E:\RealizaTi\ar-v3-dashboard\target
```

`E:\HiveLLM\Cortex\target` — the single largest directory on the machine, 172 GB,
57% of the total — **has no `CACHEDIR.TAG` at all**. This matches the known Cargo
behaviour that the tag is only written when Cargo creates the directory and is
skipped when the directory already exists (rust-lang/cargo #12441, #14281) — for
instance when rust-analyzer got there first, which is highly likely here given
F-052.

**Impact.** A `CACHEDIR.TAG`-gated sweeper would silently skip the single biggest
prize on the machine and report success. This is the most consequential
correction in the analysis, and it invalidates the recommendation that both this
file and the prior-art survey (F-041) originally reached.

Required discriminator, checked immediately before any deletion and never cached
from an earlier scan:

```
is_cargo_target_dir(p) :=
      exists(p/CACHEDIR.TAG)            # when present, authoritative
   || exists(p/.rustc_info.json)        # note: underscore, not hyphen (F-041)
   || any(child of p is a dir containing .fingerprint/)   # structural fallback
```

The structural fallback is the one that must never be dropped: it is
layout-derived rather than marker-derived, so it also survives the directories
where Cargo simply never wrote a marker. Note that F-041 warns the layout is
being restructured, so this test needs a version-aware second form for the new
build-dir layout.

Do **not** use `CACHEDIR.TAG`'s absence as evidence *against* a directory being a
target dir. Do use its presence as sufficient evidence *for*.

**Confidence: high** (direct measurement across all 18 directories plus 5
sampled false positives).

---

## F-056 — Scanning must be metadata-only to avoid destroying its own best signal

Every measurement in this analysis enumerated files via directory metadata
(`FileInfo.Length`) and never opened a file. That is what kept the `atime`
evidence in F-008 intact and interpretable.

**Impact.** A cleaner that reads artifact contents — to hash them, to parse a
`.d` file, to verify an ELF/PE header — sets `atime` on everything it touches and
permanently destroys the access-recency signal for itself and for every other
tool. If content must be read (e.g. parsing `.fingerprint/*.json`, which is
legitimate and cheap), it should be confined to the small `.fingerprint`
directory (0.01 GB, F-005) and never applied to `deps/` or `incremental/`.

**Confidence: high.**

---

## F-057 — Failure modes ranked by severity

| # | failure | trigger | severity | mitigation |
|---|---|---|---|---|
| 1 | delete artifacts of a live build | no lock held (F-051) | **critical** — corrupt/failed build, blamed on Cargo | hold the Cargo lock (F-054) |
| 2 | delete a non-target directory | name-based discovery (F-055) | **critical** — source loss | require `CACHEDIR.TAG` |
| 3 | half-deleted incremental dir | recursive delete hits an open file (F-053) | high — rustc may see a corrupt session | per-file deletes; remove dir only when empty |
| 4 | build stalls for the sweep's duration | spawn from build.rs (F-022) | high — silent, blamed on the compiler | never spawn naively; use the OS scheduler |
| 5 | every build recompiles | forced build-script re-run (F-020) | high — silent, permanent slowdown | build.rs must never force re-run |
| 6 | IDE hangs during a sweep | lock held too long (F-052/F-054) | medium | bounded per-directory sweeps; idle scheduling |
| 7 | cold rebuild after cleanup | age-based policy on a dormant project (F-007) | medium — the exact thing this project exists to avoid | identity-based policy (F-044) |
| 8 | atime signal destroyed | content-reading scan (F-056) | low, but self-inflicted and irreversible per-file | metadata-only enumeration |
| 9 | follow a junction out of the tree | naive recursion | medium | skip reparse points explicitly |

**Impact.** Failures 4 and 5 deserve particular emphasis: they are *silent*. The
user experiences a slower machine and never connects it to the disk tool they
installed months ago. A disk tool that quietly taxes every build has negative
net value, and unlike a corrupted build it produces no signal that anything is
wrong. Both must be structurally impossible in the design, not merely avoided by
care.

**Confidence: high.**
