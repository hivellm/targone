# 09 — Cargo's upstream roadmap: what ships, what's accepted, what never will

What Cargo itself has shipped, accepted, or refused — the timeline that bounds
Targone's scope and lifespan. Baseline: stable = Cargo/Rust **1.97.0**
(2026-07-09); 1.98 ~2026-08-20; 1.99 ~2026-10-01. There is **no GC RFC** — the
work runs through a HackMD design doc plus tracking issue rust-lang/cargo#12633
(RFC 3537 is the MSRV resolver, a common mix-up).

---

## F-065 — Cargo's shipped auto-GC will keep covering only `$CARGO_HOME`

| Version | Date | What shipped |
|---|---|---|
| 1.78 | 2024-05 | Last-use tracking for `$CARGO_HOME` |
| **1.88** | 2025-06 | **Automatic global-cache GC**: network files unused 3 months / local files 1 month removed; `cache.auto-clean-frequency` default `"1 day"` |
| 1.91 | 2025-10 | `build.build-dir` stabilized (artifact-dir/build-dir split) |
| 1.93 | 2026-01 | `target/package` gets CACHEDIR.TAG |
| 1.96 | 2026-05 | `cargo clean` validates CACHEDIR.TAG before deleting; build-lock split (`.cargo-build-lock` / `.cargo-lock` / `.cargo-artifact-lock`) |

`cargo clean gc` (`-Zgc`) exists on nightly, still `$CARGO_HOME`-scoped, and is
explicitly **not proposed for stabilization** (#13060, `S-needs-design`).

**Impact.** Confirms F-009 from the demand side: the 1.88 auto-GC covers the
3.5 GB slice of this machine's problem, not the 300 GB slice, and upstream has
no stabilization path that changes that. Waiting is not a strategy. The 1.96
CACHEDIR.TAG validation is worth copying as a discovery/refusal guard (F-055
reached the same conclusion independently).

**Confidence: high** (changelog + tracking issues).

---

## F-066 — Whole-target-dir GC is accepted upstream (#13136) — same registry model as F-058, years out

**#13136 "Garbage collect whole `target/`"** is open, `S-accepted`, assigned.
Its model: a GC database of *(root manifest path, target dir, timestamp)* —
noting neither field can be a primary key (many workspaces ↔ one target dir) —
with modes for unused-for-X, delete-all post-toolchain-upgrade, and leaked dirs
of deleted workspaces. It is **whole-directory** GC only; the intra-target
problem (#5026, open since 2018) stays blocked on the layout rewrite. The
maintainers' own diagnosis of why (Inside Rust, cycle 1.92): *"if we were to GC
the content, we'd need to track individual files for a build unit."*

**Impact.** Upstream independently converged on the registry-of-projects shape
that F-058 proposes — validation of the design, and a compatibility target
(keep our registry semantics close to theirs). It also bounds long-term scope:
our durable differentiation is the intra-target tiers (F-049) and Windows
polish, not rediscovering dead target dirs, which Cargo will eventually do
itself.

**Confidence: high** (issue state verified 2026-08-18).

---

## F-067 — Post-build hooks: refused for a decade, direction is away from build scripts

- #545 "post-build script execution": open since **2014**, `E-hard`,
  `S-needs-design`.
- RFC 1777 (post-build scripts): **rejected after FCP in 2017** — "possible
  with a `cargo-something` command — try that first."
- #9661 (give build scripts the target-dir path): **`S-propose-close`**;
  ehuss: build scripts' "only interaction should be through the `OUT_DIR`."
- 2025 direction (#14948) is explicitly to *reduce* what build scripts do.

**Impact.** The two hooks that would let a dependency crate clean safely —
a post-build trigger, or knowledge of the target dir — are both refused, with
stable multi-year rationale. This closes the door permanently on the "the
dependency itself cleans" reading and confirms the beacon/engine split (F-058)
is not a workaround but the intended extension model (`cargo-<name>`
subcommand).

**Confidence: high.**

---

## F-068 — Cross-workspace shared build cache: funded 2026 goal, stable horizon 2027+ — do not compete with it

**Rust Project Goal 2026 "Cargo cross workspace cache"** — accepted, ~$30k AWS
funding, owner ranger-ross, champion Ed Page: a content-addressed cache giving
"the benefits of a shared `CARGO_TARGET_DIR` out of the box with no
configuration." The 2026 plan is an initial **nightly** cache for basic crates
only (no build scripts, no proc-macros); **stabilization is explicitly out of
scope for the goal period**; no `-Z` flag exists yet. Prerequisites still open:
nondeterministic codegen at `codegen-units=16` (rust#128675) and Cargo's own
metadata files not being byte-stable (#16693).

**Impact.** The *horizontal* dimension of the problem (N projects compiling the
same deps N times) is being solved upstream on a 2027+ horizon. Targone should
not build a content-addressed artifact cache — it would be obsolete on arrival
and depends on reproducibility work only rustc can do. The measured Policy A
(F-049) doesn't need it: 85.7% reclaim is vertical. Horizontal remains
out-of-scope-by-choice, revisited when the upstream cache stabilizes.

**Confidence: high** for goal status; timeline is upstream's own estimate.

---

## F-069 — `-Zmtime-on-use` is the sanctioned freshness signal — exploit when present, never require

Cargo's own words (#7150): *"an experiment to have Cargo update the mtime of
used files to make it easier for tools like cargo-sweep to detect which files
are stale."* Nightly-only, settable as `unstable.mtime_on_use` in config,
**silently ignored on stable**.

**Impact.** Upstream acknowledges exactly the signal gap measured in F-006/F-007
(mtime = produced, not used) and offers the fix only on nightly. For nightly
users the flag upgrades mtime into a true last-use signal — worth detecting and
using to *refine* keep-newest policies. On stable it changes nothing, so no
policy tier may depend on it. Same posture as atime (F-033/F-008): optimiser,
never a correctness dependency.

**Confidence: high.**

---

## F-070 — Free size levers upstream already provides — surface as advice, never depend on

- **`-Zno-embed-metadata`** (#15495): stops duplicating metadata into both
  `.rlib` and `.rmeta`. Measured upstream (Kobzol, hyperqueue): **release
  −36%**, dev −9..18%. Nightly-only; the team is weighing a default.
- `[profile.dev.package."*"] debug = 0` (or `"line-tables-only"`): drops
  dependency debuginfo — though F-004 shows on Windows-MSVC the PDB bloat
  survives even `line-tables-only` (it is per linked artifact, not per rlib).
- `CARGO_INCREMENTAL=0` in CI; `-Zbuild-analysis` (JSONL build logs with
  rebuild reasons in `$CARGO_HOME/log/`) as a possible future activity signal.

**Impact.** Belongs in the Phase 5 advice output (08): `cargo targone report`
can quantify what each lever would save on the actual directory it just
scanned, printing copy-paste config. Advice only — Targone never edits a user's
manifest (consistent with F-062's trust posture).

**Confidence: high** for the flags' existence and measured effects; per-project
savings vary.
