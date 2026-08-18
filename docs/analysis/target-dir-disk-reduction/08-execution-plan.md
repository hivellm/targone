# 08 — Execution plan

Sequenced so that **the measured win lands in Phase 2** and the riskiest,
least-valuable component (the build-script beacon) lands last. Every phase ends
with something usable; nothing deletes a byte before Phase 2, and nothing
deletes a byte unsupervised before Phase 3.

Target for the whole plan: **300.8 GB → ~43 GB** on this machine, verified by
re-running the measurement scripts from `01-measurements.md`.

---

## Phase 0 — De-risking spikes (no product code)

Answers the questions in F-064 that gate design decisions. Timeboxed; each
produces a written result, not a feature.

| # | spike | question | blocks |
|---|---|---|---|
| 0.1 | **Lock under load** | Hold `.cargo-build-lock` for 1/10/60 s while rust-analyzer runs in 3 workspaces. Largest sweep unit that stays imperceptible? | Phase 2 |
| 0.2 | **Unix lock + unlink probes** | Repeat F-050/F-053 on Linux and macOS: is `flock` exclusion effective against Cargo; does `unlink` of an open artifact corrupt a build? | Phase 2 |
| 0.3 | **`.fingerprint` liveness** | Resolve the cargo-sweep / cargo-mark-sweep contradiction (F-064.4). Does every `.fingerprint` hash appear in a `deps/` filename? My data says 3,569 vs 3,371 | Phase 2 policy tier 4 |
| 0.4 | **Incremental identity parsing** | Is `name-<disambiguator>` grouping robust, including crate names containing `-` and non-Cargo-generated dirs? | Phase 1 |
| 0.5 | **Layout detection** | Minimum assumption set; detect `build.build-dir` / `-Zbuild-dir-new-layout` and fail closed | Phase 1 |
| 0.6 | **Scheduler registration** | Task Scheduler / systemd user timer / launchd: rights needed, idempotent registration, behaviour without rights | Phase 3 |

**Exit criteria.** 0.1-0.5 answered in writing. 0.6 may run in parallel with
Phase 1-2. If 0.2 shows `flock` cannot exclude Cargo on Unix, the sweep protocol
(F-061) needs redesign before Phase 2 — treat that as a stop-and-rethink.

---

## Phase 1 — `targone-core` + read-only reporting

Everything except deletion. Ships as a usable disk-analysis tool.

**Deliverables**

- `targone-core`: discovery (`CACHEDIR.TAG`-gated, reparse-points skipped,
  filesystem-type detection), layout model behind one module (F-041),
  metadata-only enumeration (F-056), classification into the F-049 tiers.
- `cargo-targone report` — per target dir: total, per-pool breakdown,
  **reclaimable bytes per tier**, and the projected residual.
- Reproduce the numbers in `01`/`05` on this machine as the acceptance test.

**Exit criteria**

- `cargo-targone report` reproduces the measured aggregate within ±2%:
  total 300.8 GB, incremental 201.8, deps 88.4, build 3.2, Policy A 257.7 GB.
- Finds all 18 target dirs; classifies **zero** of the 30 LLVM/GCC `Target`
  directories as sweepable (F-055).
- Zero file opens outside `.fingerprint/` — assert this in a test by checking
  `atime` is unchanged after a full scan (F-056).

**Why first.** It is the whole analysis made executable and re-runnable, it
carries no deletion risk, and it makes every later phase measurable rather than
asserted.

---

## Phase 2 — Deletion with the lock protocol (Policy A, tiers 1-4)

The phase that reclaims the 257.7 GB.

**Deliverables**

- Sweep protocol exactly as F-061: verify → refuse network FS → acquire
  `.cargo-build-lock` (prefer `cargo-util`) → classify → per-file delete
  tolerating sharing violations → release. One profile dir per acquisition.
- Policy tiers 1-4 (F-049): incremental keep-newest-1-per-crate; deps
  newest-hash-only; build keep-newest-per-package; orphan fingerprints **only
  if spike 0.3 clears it**.
- `--dry-run` **default**; `--apply` required to delete; an append-only audit log
  of every path removed with reason and size.
- Windows-hardened deletion via the `remove_dir_all` crate (F-037).
- Fail-open invariant, asserted in tests: anything not positively classified as
  superseded is kept (F-060).

**Exit criteria**

- Concurrency test: sweep in a loop while `cargo build` and `cargo check` run
  continuously against the same target dir, ≥100 iterations, **zero** build
  failures and zero corrupt artifacts.
- Recovery test: after a full Policy A sweep, `cargo build` performs **zero**
  recompilations on an unchanged tree (the F-042 result, at project scale).
- On this machine: 300.8 GB → ≤45 GB, and a subsequent full `cargo test` on
  Cortex succeeds.
- Interrupt test: kill the sweeper mid-delete; the target dir remains buildable.

**Risk.** This is where the project's entire reputational risk sits (F-051,
F-057). The concurrency test is not a formality — it is the deliverable.

---

## Phase 3 — Recurrence via the OS scheduler

Turns a tool someone must remember into one that runs itself — the actual gap
identified in F-010.

**Deliverables**

- `cargo-targone schedule install|status|uninstall`: idempotent registration
  with Task Scheduler / systemd user timer / launchd; idle-triggered, daily.
- Registry file at `$CARGO_HOME/targone/registry.jsonl` (append-only) plus
  configurable scan roots; discovery reads both (F-059).
- Global budget as an *ordering and stopping* function over **reclaimable**
  bytes only (F-048, F-034); directories processed descending by reclaimable
  size (F-001).
- `TARGONE_DISABLE=1` and CI detection → hard no-op (F-062.10).

**Exit criteria**

- Survives reboot; runs unattended for a week; aggregate stays under budget.
- A dormant project's directory is still swept (proves registry durability,
  F-017).
- Uninstall leaves no scheduler entry and no daemon.

---

## Phase 4 — The `targone` beacon crate

The `cargo add targone` adoption path. Last, because F-059 shows it is worth
a few percent and F-062 shows it is the easiest component to make actively
harmful.

**Deliverables**

- `targone` crate: `build.rs` only. Walks up from `OUT_DIR` to `CACHEDIR.TAG`
  (F-021), appends `{root, toolchain, rustflags, profile, first_seen}` to the
  registry, ensures a scheduler entry exists, exits.
- Hard invariants, each with a test: emits **no** `rerun-if-changed` pointing at
  a missing path (F-020); spawns **no** process (F-022); deletes **nothing**
  (F-036); total build-script runtime < 50 ms; output byte-identical across runs.

**Exit criteria**

- Warm-build benchmark: adding `targone` to the probe project changes median
  warm build time by < 5 ms against the 78 ms baseline (F-020), and causes
  **zero** recompilations across 20 consecutive builds.
- Adding it to Cortex does not increase build time measurably.
- Works when the registry path is unwritable (degrades silently, never fails a
  build).

---

## Phase 5 — Optional tiers and alternative triggers

Each independently switchable, each off by default.

- **Tier 5 — drop all PDBs** (+13.0 GB, 300.8 → 30.1 GB, 10.0x). Free per F-042;
  costs symbolisation of already-built binaries. Windows-relevant only.
- **Tier 6 — dormant directories.** Target dirs unbuilt for > N days → full
  reclaim (F-043's legitimate use of absolute age).
- **Tier 7 — uninstalled toolchains** (F-046): artifacts from toolchains no
  longer present, using cargo-sweep's dual-hash keep-set and fail-open
  discipline (F-032). Accept the maintenance burden knowingly.
- **PATH shim trigger** (F-040): post-build cleanup with ideal timing, opt-in,
  clearly labelled as higher blast radius.
- **Advice output**: report profile settings that inflate `target/` —
  e.g. `[profile.dev.package."*"] debug = 0` to drop debuginfo for dependencies
  while keeping it for the user's own crates (F-027). Advice only; never
  auto-edit a user's manifest.

---

## Sequencing summary

| phase | delivers | reclaims | risk |
|---|---|---|---|
| 0 | six written answers | — | none |
| 1 | `report`, read-only | — | none |
| **2** | **Policy A deletion under lock** | **257.7 GB (85.7%)** | **high — concurrency** |
| 3 | unattended recurrence | keeps it bounded | medium |
| 4 | `cargo add targone` | a few % more coverage | low, if invariants hold |
| 5 | opt-in tiers | +13.0 GB → 10.0x | user-chosen |

Phases 1-2 are the project. Phase 3 is what makes it stick. Phases 4-5 are
polish — and if Phase 4 never ships, nothing measured in this analysis is lost.
