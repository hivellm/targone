# 01 — Measurements: what is actually on this disk

> All numbers measured on the primary dev machine (Windows 10 Pro 19045, NTFS,
> cargo 1.97.1 / rustc 1.97.1) on 2026-08-18. Sizes are logical file sizes
> summed via `System.IO.Directory.EnumerateFiles` (reparse points skipped, so no
> double counting through junctions). Scripts used are listed at the end.

The problem statement estimated "terabytes in aggregate". The measured figure on
the drive scanned is **300.5 GB across 18 Cargo `target/` directories**, and the
shape of that 300 GB is more informative than the total: it is not evenly spread,
and the dominant consumer is not the one most people assume.

---

## F-001 — 300.5 GB in 591,462 files across 18 target dirs; top 3 projects hold 91%

`E:\HiveLLM` + `E:\RealizaTi`, all genuine Cargo target directories (see F-055
for how they were distinguished from 30 same-named LLVM/GCC source directories —
the naive `CACHEDIR.TAG` test would have missed the largest one on this list):

| GB | files | path |
|---:|---:|---|
| **172.0** | 392,566 | `E:\HiveLLM\Cortex\target` |
| **56.7** | 88,308 | `E:\HiveLLM\Thunder\rust\target` |
| **44.9** | 46,152 | `E:\RealizaTi\ar-v3-dashboard\target` |
| 8.6 | 15,105 | `E:\RealizaTi\ar-database-sync\target` |
| 7.7 | 15,279 | `E:\HiveLLM\Nexus\sdks\rust\target` |
| 2.5 | 7,566 | `E:\HiveLLM\VecLite\crates\veclite-py\target` |
| 2.1 | 5,220 | `E:\HiveLLM\VecLite\crates\veclite-node\target` |
| 2.1 | 4,456 | `E:\HiveLLM\Vectorizer\sdks\rust\target` |
| 1.3 | 7,353 | `E:\RealizaTi\ar-v3\target` |
| 0.5 | 1,911 | `E:\HiveLLM\Nexus\scripts\interop\clients\rust\target` |
| 0.5 | 1,463 | `E:\HiveLLM\Cortex\scripts\nexus_smoke\target` |
| 0.5 | 2,171 | `E:\HiveLLM\VecLite\crates\veclite-wasm\target` |
| 0.5 | 1,237 | `E:\HiveLLM\VecLite\fuzz\target` |
| 0.4 | 1,875 | `E:\HiveLLM\Nexus\benchmarks\ldbc-snb\target` |
| 0.2 | 768 | `E:\HiveLLM\Fluxum\crates\fluxum-bench\spacetimedb-module\target` |
| 0.0 | 32 | `E:\HiveLLM\Nexus\crates\nexus-core\target` |
| 0.0 | 0 | `E:\HiveLLM\Lexum\crates\lexum-core\target` (empty husk) |
| 0.0 | 0 | `E:\HiveLLM\Synap\crates\synap-server\target` (empty husk) |
| **300.5** | **591,462** | **TOTAL** |

**Impact.** The distribution is extremely skewed — a heavy-tail. A tool that only
fires on projects above a size threshold would recover 91% of the bytes while
touching 3 of 18 projects. Conversely, a per-project quota applied uniformly
(e.g. "5 GB each") would be both useless for the 15 small projects and
gratuitously destructive for the 3 large ones. **Budgeting must be global (or
per-project but adaptive), not a uniform per-project constant.**

**Confidence: high** (direct measurement).

---

## F-002 — `incremental/` is 65-74% of every large target dir — not `deps/`

Level-2 breakdown of the three largest:

```
E:\HiveLLM\Cortex\target
     171.8 GB  392,379 f  debug
         0.7 GB    1,876 f    └ build
        42.4 GB    9,234 f    └ deps
       127.0 GB  366,528 f    └ incremental      <-- 74% of the project
       0.01 GB   14,696 f    └ .fingerprint
       0.2 GB       59 f  release
E:\HiveLLM\Thunder\rust\target
      55.5 GB   85,247 f  debug
        16.7 GB    4,624 f    └ deps
        38.1 GB   71,664 f    └ incremental      <-- 69%
       1.2 GB    3,057 f  release
E:\RealizaTi\ar-v3-dashboard\target
      44.9 GB   46,145 f  debug
        13.9 GB    5,127 f    └ deps
        30.1 GB   30,472 f    └ incremental      <-- 67%
```

`incremental/` accounts for **195.2 GB of the 272.4 GB** in these three projects.

**Impact.** This inverts the usual mental model ("target/ is full of compiled
dependencies"). It also changes the risk calculus dramatically: `incremental/`
is a *pure accelerator cache* for the local crates. Deleting it does **not**
invalidate a single `.rlib` in `deps/`; the next build simply performs a
non-incremental compile of whatever changed. Losing `deps/` means recompiling
hundreds of dependencies; losing `incremental/` means one slower compile of your
own crates. **The largest pool of bytes is also the cheapest to reclaim.**

**Confidence: high** (direct measurement).

---

## F-003 — `incremental/` holds 8,113 directories for only 196 distinct crates (41x duplication)

Inside `E:\HiveLLM\Cortex\target\debug\incremental`:

- **8,113** crate directories, named `<crate_name>-<disambiguator>`
- only **196** distinct crate names
- worst offenders: `cortex_api` 206 dirs / 23.79 GB, `cortex_workers` 140 dirs /
  31.49 GB (avg 230 MB *each*), `cortex_mcp_server` 131 dirs
- each crate dir contains 1-2 `s-*` session subdirectories (rustc does prune
  *within* a crate dir; it never prunes *across* them)

Only the newest directory per crate name can possibly be reused by the next
build. The other 7,917 are dead the moment a new disambiguator appears.

**Impact.** Simulated policy on this directory:

| policy | keeps | frees |
|---|---:|---:|
| keep newest **1** dir per crate name | 4.0 GB (196 dirs) | **123.1 GB (96.9%)** |
| keep newest **2** dirs per crate name | 6.0 GB (392 dirs) | 121.1 GB (95.3%) |
| keep newest **3** dirs per crate name | 7.4 GB (588 dirs) | 119.7 GB (94.2%) |

A `keep-newest-1-per-crate` rule reclaims 96.9% of the single largest pool of
bytes on the machine while losing nothing a future build can use.

**Confidence: high** (direct measurement + simulation).

---

## F-004 — 90% of `deps/` is `.pdb` + `.exe`; the artifacts that actually keep builds warm are 4.1 GB

`E:\HiveLLM\Cortex\target\debug\deps` — 9,233 files, 42.4 GB, cross-tabulated by
extension against "was this file ever read after it was written?"
(`LastAccessTime - LastWriteTime > 1h`):

| ext | total | re-read | never re-read | files |
|---|---:|---:|---:|---:|
| `.pdb` | **25.3 GB** | **0.0 GB (0 files)** | 25.3 GB | 730 |
| `.exe` | **12.9 GB** | 0.1 GB (28 files) | 12.8 GB | 696 |
| `.rlib` | 2.8 GB | 1.0 GB (381 files) | 1.8 GB | 492 |
| `.rmeta` | 1.3 GB | 0.7 GB (777 files) | 0.6 GB | 2,658 |
| `.dll` | 0.1 GB | 0.1 GB (32 files) | 0.0 GB | 34 |
| `.lib`/`.exp`/`.d` | ~0.05 GB | — | — | 4,623 |

**Impact.** This is the most important structural finding in the whole analysis.
`.rlib` + `.rmeta` — the *only* things a subsequent build consumes as input —
total **4.1 GB out of 42.4 GB (9.7%)**. Everything else in `deps/` is terminal
output: linked test binaries and their Windows debug symbol files, which nothing
ever reads back. A GC that deletes stale `.pdb`/`.exe` and preserves every
`.rlib`/`.rmeta` frees ~90% of `deps/` at essentially **zero** warm-build cost:
the worst case is a re-link (seconds), never a re-compile (minutes).

Note this is with `[profile.dev] debug = "line-tables-only"` already configured in
Cortex's `Cargo.toml` — the `.pdb` bloat is *not* the result of a careless debug
setting. It is inherent to `x86_64-pc-windows-msvc`, which always emits a
separate PDB per linked artifact.

**Confidence: high** (direct measurement).

---

## F-005 — `.fingerprint/` dir names and `deps/` filename hashes share one hash namespace (3,371/3,371 overlap)

`.fingerprint/<pkg>-<hash>/` and `deps/<lib><name>-<hash>.<ext>` use the *same*
16-hex hash. Measured on Cortex `debug/`:

```
.fingerprint dirs                      : 3,569 (3,569 distinct hashes)
deps distinct extra-filename hashes    : 3,371
overlap (deps hash has fingerprint dir): 3,371   (100%)
bytes in deps with no fingerprint dir  : 0.0 GB
```

A fingerprint directory looks like:

```
adler2-18312cf7466f4234/
    dep-lib-adler2  invoked.timestamp  lib-adler2  lib-adler2.json
ahash-3a1cf4a6f4ae5ae7/
    run-build-script-build-script-build  run-build-script-build-script-build.json
```

`invoked.timestamp` contains the literal text
`This file has an mtime of when this was started.`

**Impact.** There is an exact, Cargo-maintained join key between "unit of work"
and "files on disk". A GC does **not** need to guess from filename heuristics:
it can enumerate `.fingerprint/`, derive the hash set, and treat `deps/` files by
hash. Equally important, the reverse is *not* true — `.fingerprint/` is itself
never pruned (3,569 entries for a project with ~200 live units), so **presence of
a fingerprint dir does not mean "live"**. Liveness has to come from timestamps,
not from existence.

**Confidence: high** (direct measurement).

---

## F-006 — `invoked.timestamp` records "last actually compiled", not "last used"

The 3,471 `invoked.timestamp` files in Cortex's `debug/.fingerprint` cluster onto
exactly three days, in minute-resolution bursts that correspond to build
sessions:

```
2026-08-02 :   786 units      2026-08-02 05:24 : 328 units
2026-08-05 : 1,848 units      2026-08-09 23:09 : 301 units
2026-08-09 :   837 units      2026-08-05 13:42 : 300 units
```

If Cargo refreshed the stamp on every build, all 3,471 would carry the most
recent date. They do not — a unit that was fresh (cache hit) keeps its old
timestamp.

**Impact.** `invoked.timestamp` (and `mtime` generally) answers "when was this
built", **not** "is this still in use". A dependency compiled on 08-02 that is
still linked into every build today looks 16 days stale by mtime. This is the
precise reason a naive age policy is unsafe — see F-007.

**Confidence: high** (direct measurement; mechanism corroborated in 04-prior-art).

---

## F-007 — A pure age threshold degenerates into `cargo clean`

Policy simulation on Cortex `debug/deps` (42.4 GB), evaluated 2026-08-18 against
a directory whose last build was 2026-08-09:

| threshold | freed by **mtime** | freed by **atime** |
|---|---:|---:|
| older than 3d | 42.4 GB (100%) | 42.4 GB (100%) |
| older than 7d | 42.4 GB (100%) | 42.4 GB (100%) |
| older than 14d | 1.1 GB (2.7%) | 0.1 GB (0.2%) |
| older than 30d | 0.0 GB | 0.0 GB |

And on `debug/incremental` (127.0 GB): `age > 7d` frees **100%**, `age > 14d`
frees 67.5%, `age > 30d` frees 57.3%.

**Impact.** Wall-clock age is a *cliff function*, not a gradient. Any project you
have not touched for longer than the threshold loses its entire warm cache — the
exact failure mode of `cargo clean` that this project exists to avoid, just
arriving automatically instead of on request. Meanwhile a project built daily is
never cleaned at all, no matter how bloated. **Age must be measured relative to
the target directory's own most recent build, not to wall-clock now** — or
replaced by a relative rule (keep newest N per key), which has no cliff.

**Confidence: high** (direct simulation).

---

## F-008 — `atime` is live on this machine and separates inputs from outputs

`fsutil behavior query disablelastaccess` reports `DisableLastAccess = 2`
(system-managed). Empirically, access times *are* being updated and *do* differ
from write times:

```
libadler2-18312cf7466f4234.rlib   W=02/08 09:49:37   A=02/08 09:49:37   (never re-read)
libahash-65ef5888ecb646c3.rlib    W=02/08 06:27:56   A=09/08 23:00:21   (re-read 7d later)
libahash-6ca8b9bc33b96035.rlib    W=05/08 10:37:14   A=09/08 23:09:35   (re-read)
```

Across `deps/`: 1,531 files (1.8 GB) show a later atime than mtime; 7,702 files
(40.6 GB) were never re-read after being written.

**Impact.** `atime` is the only *directly observed* "still in use" signal
available without instrumenting Cargo. Combined with mtime it yields a two-axis
classification — `mtime` = when produced, `atime` = when last consumed — which is
what F-004's input/output split is actually built on. Caveats that a design must
handle: NTFS updates atime with ~1 hour granularity and the behaviour is
system-managed (can be turned off); Linux defaults to `relatime` (updated when
older than mtime or >24h stale — sufficient here); `noatime` mounts and some
container setups provide nothing. **atime is a valuable optimiser, never a
correctness dependency.**

**Confidence: high** for the measurement; **medium** for cross-platform
generalisation (Linux/macOS not measured here).

---

## F-009 — Cargo's existing garbage collection covers 3% of the problem

| location | size |
|---|---:|
| `C:\Users\Bolado\.cargo` (registry cache + src + git) | **3.5 GB** |
| `C:\Users\Bolado\.rustup\toolchains` (7 toolchains) | **7.2 GB** |
| all `target/` directories | **300.5 GB** |

Per-toolchain: stable 2.0, nightly 2.1, 1.93.1 0.9, nightly-2026-02-27 0.6,
1.87 0.5, 1.88 0.5, nightly-2025-01-01 0.4 GB.

**Impact.** Cargo's shipped auto-GC targets `$CARGO_HOME` — 3.5 GB, i.e. **1.2%**
of the bytes at issue; even counting toolchains the entire globally-managed
footprint is 10.7 GB, 3.4% of the target directories. Waiting for upstream Cargo
to solve this is not a strategy on this timescale. The seven installed toolchains
do however confirm that toolchain-keyed staleness (artifacts built by a toolchain
you no longer use) is a real dimension, not a hypothetical one.

**Confidence: high** (direct measurement).

---

## F-010 — `cargo-sweep` is already installed and has visibly never been run

`C:\Users\Bolado\.cargo\bin` contains `cargo-sweep.exe`, alongside
`cargo-nextest`, `cargo-llvm-cov`, `cargo-audit`, `cargo-deny`, `cargo-fuzz`,
`cargo-tarpaulin` and others. Yet Cortex carries 8,113 incremental directories
spanning 60 days.

**Impact.** This is the strongest available evidence for the problem statement's
"manual and forgettable" claim: the correct tool was installed and then not used.
It reframes the project's core value proposition. Targone's differentiator is
**not** a better deletion algorithm — `cargo-sweep` already has a decent one.
It is *automatic invocation*: making the cleanup happen without anyone deciding
to run it. Any design that still requires a human to remember a command has
already failed, regardless of how good its policy is.

**Confidence: high** (direct observation).

---

## Environment facts recorded for later sections

| fact | value | relevance |
|---|---|---|
| cargo / rustc | 1.97.1 (2026-06-30 / 2026-07-14) | recent; has `$CARGO_HOME` GC, no target GC |
| host triple | `x86_64-pc-windows-msvc` | PDB-per-artifact; delete-while-open restrictions |
| filesystems | all **NTFS** (C/E/F/G/W) | hardlinks yes; **no ReFS block-cloning** |
| free space | E: 422.6 GB free of 953.9 GB | 300 GB of target/ = 31% of the volume |
| `CARGO_TARGET_DIR` | unset | no shared target dir today |
| global `$CARGO_HOME/config.toml` | absent | a global config drop-in is available and unoccupied |
| lock files (cargo 1.97) | `target/<profile>/.cargo-lock`, `.cargo-build-lock`, `.cargo-artifact-lock` | three locks, in the **profile** dir |
| `target/.rustc_info.json` | 2,428 bytes, at target root | toolchain identity of the dir |
| installed toolchains | 7 | toolchain-keyed staleness is real |

Per-project `.cargo/config.toml` files already exist in **7** projects and are
mutually incompatible in ways that matter later (see F-018 and F-024):

- `HiveGPU` — `rustflags = ["-C","target-cpu=native"]`
- `Transmutation`, `Vectorizer` — `rustflags = ["-A","warnings"]`
- `Synap` — pins `[build] target-dir = "target"`
- `Tml/compiler/cranelift` — pins `[build] target-dir = "../../build/cranelift"`
- `HivehubCloud/apps/api` — target-specific `rustflags` with `-L` paths
- `ar-v3` — per-target `rustflags` with `target-cpu=x86-64-v3`

---

## Scripts

Reproduction scripts live in the session scratchpad (not committed):
`scan-inc.ps1` (incremental age/name breakdown), `scan-policy.ps1` (keep-N and
age simulations), `scan-atime.ps1` (atime vs mtime), `scan-live.ps1`
(extension × re-read cross-tab, fingerprint↔deps hash overlap), `scan-all.ps1`
(aggregate policy simulation, see `05-policies.md`).
