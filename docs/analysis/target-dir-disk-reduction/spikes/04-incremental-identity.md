# Spike 0.4 — incremental/ identity parsing

> Executed 2026-08-18 on the reference machine (Windows 10, NTFS). Read-only
> scan — names + metadata only — of 10,227 crate dirs / 33,974 inner entries
> across four real `incremental/` roots: Cortex debug (8,113 dirs), Thunder
> debug (952), ar-v3-dashboard debug (1,157), Cortex llvm-cov debug (5).
> Scripts in the session scratchpad (`scan-incremental.ps1`,
> `scan-incremental-v2.ps1`; JSON dumps `top-rows-v2.json` etc.).

## Verdict

**The grammar is unambiguous on real data: zero violations at either level
across all 10,227 dirs.** Confidence: high for these workspaces (exhaustive
scan); medium for other toolchains/OSes (one machine, one rustc era).

### Parsing grammar (all patterns anchored, ASCII)

| Level | Entry | Pattern | Observed |
|---|---|---|---|
| top | crate dir (dir) | `^([A-Za-z0-9_]+)-([0-9a-z]+)$` — group 1 = crate name, group 2 = disambiguator | 10,227/10,227 match; exactly one `-` in every name; crate segment never contains `-` (cargo/rustc forbid hyphens in crate names) |
| top | anything else | **unrecognized** | 0 observed (no files, no odd names) |
| inner | finalized session (dir) | `^s-[0-9a-z]+-[0-9a-z]+-[0-9a-z]+$` (`s-<timestamp>-<random>-<svh>`) | 17,057; widths uniformly 10/7/25 chars |
| inner | in-progress session (dir) | `^s-[0-9a-z]+-[0-9a-z]+-working$` | 39 (crashed/interrupted builds) |
| inner | session lock (file) | `^s-[0-9a-z]+-[0-9a-z]+\.lock$` | 16,878 |
| inner | anything else | **unrecognized** | 0 observed |

Match segments as `[0-9a-z]+`, not fixed widths — the observed widths
(disambiguator 13 = zero-padded base36 u64; timestamp 10; random 7; svh 25)
are uniform here and usable as a warning-level sanity check, but should not
be load-bearing across toolchains. Confidence in the width claim: high
empirically (25.2% of disambiguators lead with `0`, matching the 25.7%
expected from a zero-padded uniform u64), medium as an invariant.
Note the order of dir checks matters: `-working` is itself `[0-9a-z]+`, so
test the working pattern before the finalized one.

### Newest-selection key

**Use the crate dir's own mtime.** Compared against "newest session-dir
mtime inside" on **all 334** multi-dir crate groups (not just 20): the two
keys pick the same winner **334/334**, and in every sampled group the two
timestamps are identical to the second — NTFS updates the parent dir's
mtime when a session dir is created/renamed inside it, so dir mtime *is*
newest-session mtime. Dir mtime needs one stat and no recursion.
Tie-break (same-second ties do occur across dirs of one build run): decode
the base36 `<timestamp>` segment of the newest session dir name — rustc's
own ordering key, immune to filesystem timestamp quirks. Confidence: high
on this data; caveat — a `target/` restored by a mtime-clobbering copy
breaks both keys equally (the session-name timestamp then remains the only
trustworthy key).

### Refusal rule (asymmetric, fail-closed)

1. **Unrecognized top-level entry → skip the entry** (never delete, never
   group it; report). It cannot poison siblings — well-formed dirs are still
   grouped and swept.
2. **Unrecognized entry inside a deletion candidate → skip the whole crate
   dir** (deletion is recursive `rm -rf`; unknown content means the dir is
   not provably pure cache). Fired 0 times on this data.
3. **Locks gate deletion:** before removing a crate dir, exclusively acquire
   every `.lock` in it (rustc holds them for the duration of a session);
   any acquisition failure → dir in use → skip. `-working` dirs alone do
   not block (39 observed, all crash leftovers — rustc GCs them itself) —
   the lock check is what distinguishes crashed from active.
4. **`build_script_build` is excluded from keep-newest-N grouping** — see
   the trap below.

### Critical trap: `build_script_build` is not one crate

Every package's build script compiles to crate name `build_script_build`.
Observed directly: the 5 llvm-cov dirs are all `build_script_build-<disamb>`
with **identical mtimes (same build run)** — they are five *different
packages'* build scripts, not five generations of one crate. Cortex has 30
such dirs, Thunder 12. "Keep newest 1" would delete the live cache of every
package but one. Not a correctness hazard (incremental state is pure cache;
worst case a rebuild) but it breaks the "duplicates" premise. Rule: names
matching `^build_script_[A-Za-z0-9_]+$` form one group **per dir** (i.e.
never reclaimed by newest-N; age-based policy may still apply).
Checked the same hazard for test/bench/example target names (`smoke`,
`aggregator`, … are target-file stems, not package names): **zero
cross-package stem collisions in all three workspaces**, so plain crate-name
grouping is safe *here*; other workspaces could collide (two packages each
with `tests/smoke.rs`), degrading gracefully to an extra rebuild.
Confidence: high (verified against the workspaces' source trees).

### Reclaim shape

10,227 dirs collapse to 335 (project, crate) groups → keep-newest-1 removes
9,892 dirs (96.7%); with the `build_script_build` exclusion, 9,848 (96.3%).
The duplication is real and extreme: worst crates `thunder_bench` ×256,
`cortex_api` ×206, `cortex_workers` ×140, `ar_dashboard` ×51.

## Evidence

### Per-project scan summary

| Root | Crate dirs | Distinct crates | ≥1 `.lock` | >1 finalized | With `-working` | Unrecognized entries |
|---|---|---|---|---|---|---|
| Cortex `target/debug/incremental` | 8,113 | 196 | 7,905 (97.4%) | 5,801 (71.5%) | 7 | 0 |
| Thunder `rust/target/debug/incremental` | 952 | 23 | 952 (100%) | 374 (39.3%) | 13 | 0 |
| ar-v3-dashboard `target/debug/incremental` | 1,157 | 115 | 1,157 (100%) | 659 (57.0%) | 0 | 0 |
| Cortex `target/llvm-cov-target/debug/incremental` | 5 | 1 (`build_script_build`) | 5 (100%) | 5 | 0 | 0 |
| **Total** | **10,227** | **333 names** | **10,019 (98.0%)** | **6,839 (66.9%)** | **20 dirs / 39 entries** | **0** |

`release/incremental` exists but is **empty** in Cortex and Thunder, absent
in ar-v3 — expected, incremental is off by default in release. `flycheck0`
target dirs contain no `incremental/`. The llvm-cov root means discovery
must glob `**/incremental` under target roots, not assume
`target/<profile>/incremental` (consistent with spike 0.5's layout probe).

### Q1 — top-level names

- Violations of `^[A-Za-z0-9_]+-[0-9a-z]+$`: **none** (list is empty).
- Hyphen count per name: 1 for all 10,227.
- Distinct crate-name segments: 333; containing `-`: 0; containing
  uppercase: 0.

### Q2 — disambiguator

- Alphabet observed across all 10,227 values: exactly `0123456789abcdefghijklmnopqrstuvwxyz`.
- Length: 13 for 10,227/10,227. Leading `0` on 2,575 (25.2%) — the
  fixed-width zero-padding signature of a base36-encoded u64 (expected
  25.7%).
- Stability per (crate, workspace): **not stable — this is the reclaimable
  duplication.** Distribution of dirs-per-crate: median ~9 (ar-v3), ~32–43
  (Cortex), long tails to 256. No disambiguator value is shared by two
  different crate names anywhere in the data.

### Q3 — inner entries

- 33,974 entries total: 17,057 finalized session dirs + 39 `-working` dirs
  + 16,878 `.lock` files. Unrecognized: **0** (no stray files, no odd
  names).
- Finalized sessions per crate dir: 0 → 9 dirs (all have a `-working`
  instead), 1 → 3,379, 2 → 6,839, **never more than 2** — rustc's own
  session GC keeps current + previous. So "sessions inside a dir" is
  rustc's job; the sweep only ever acts at crate-dir granularity.
- Locks per dir: 0 ×208, 1 ×3,181, 2 ×6,831, 3–7 ×7 (the 6–7-lock dirs are
  `thunder_bench` with 4–5 crashed `-working` sessions each).
- Orphan locks (lock with no matching session dir): **1** of 16,878
  (`bootstrap_law_extraction_it-2ves089hyorup/s-hkduqiajkj-1u4m33l.lock`,
  2026-07-14 — its session was GC'd by a later build, lock left behind).
  Grammar treats locks as standalone entries, so orphans parse fine.
- The 208 lock-free dirs are all in Cortex with mtimes clustered in one
  window (2026-06-18 20:19–20:24 UTC) — one historical event, not a pattern.
  Locks are therefore *optional* in the grammar.

### Q4 — newest-selection key

All 334 crate groups with ≥2 dirs checked: argmax-by-dir-mtime ==
argmax-by-newest-session-mtime in 334/334. 20-group sample (7 smallest-key
ar-v3 groups + 13 largest groups overall) — every row agrees and the two
timestamps are second-identical; representative rows:

| Group | Dirs | Winner disamb (both keys) | dir mtime = newest session mtime |
|---|---|---|---|
| Thunder / thunder_bench | 256 | `0xsiqha82kpc2` | 2026-07-19 13:48:03Z |
| Cortex / cortex_api | 206 | `0bpdw1jp3darm` | 2026-08-10 02:10:32Z |
| Cortex / cortex_workers | 140 | `2eqi2u1bviw0s` | 2026-08-10 02:09:48Z |
| ar-v3 / ar_dashboard | 51 | `3fm0gs3s1py6f` | 2026-08-18 07:39:00Z |
| ar-v3 / acl_enforcement | 10 | `2wryo0wwby6i2` | 2026-08-18 07:39:00Z |

(Full table: `q4-disagreements.json` is an empty list; sample generation in
the scratchpad scripts.)

### Q5 — non-Cargo junk

None. Zero top-level files, zero unmatchable names, in all four roots.

## Method & caveats

- PowerShell / .NET `EnumerateFileSystemInfos` (includes hidden), names and
  `LastWriteTimeUtc` only; no file contents opened, nothing modified.
  Scripts: scratchpad `scan-incremental.ps1` (v1, wrong session grammar —
  kept as a record of the mistake), `scan-incremental-v2.ps1` (corrected),
  plus ad-hoc aggregation over `top-rows-v2.json`.
- **v1 lesson worth keeping:** the naive guess `s-<ts>-<x>` for finalized
  sessions is wrong — finalized dirs carry *three* segments
  (`s-<ts>-<random>-<svh>`); only lock files use the two-segment base name.
  A sweep written from the rustc docs' shorthand would have flagged all
  17,057 finalized sessions as unrecognized and (under the refusal rule)
  swept nothing — fail-closed works, but the grammar above is the one
  validated against reality.
- Sample bias: three workspaces, one machine, Windows/NTFS, builds from
  2026-06 through 2026-08 (rustc stable of that era). The zero-violation
  result should be re-checked cheaply by the tool itself at runtime — the
  refusal rule *is* that recheck.
- mtime semantics: the dir-mtime == newest-session-mtime identity relies on
  the parent-directory update on child create/rename (POSIX and NTFS both
  do this). A backup/restore that rewrites mtimes would skew newest
  selection; the base36 timestamp inside session names is the fallback key.
- Cross-package collision check parsed source-tree file stems
  (`tests|benches|examples/*.rs`, hyphens normalized to `_`), not cargo
  metadata; `src/bin/` stems were not checked (none of the observed crate
  names look bin-shaped and lib/bin names share the package namespace, but
  a follow-up `cargo metadata --format-version 1` target listing would
  close that gap).
