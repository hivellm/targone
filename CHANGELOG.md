# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added — analysis & spikes (Phase 0)

- Problem statement and full solution-space analysis: 70 numbered findings
  (F-001…F-070) across 10 documents in `docs/analysis/target-dir-disk-reduction/`,
  grounded in direct measurement of the reference machine (300.5 GB across 18
  `target/` directories, 91% held by 3 projects, `incremental/` = 65–74% of
  every large dir) plus source-verified research on Cargo internals, prior-art
  tools, and the upstream GC roadmap.
- Six de-risking spikes with written results in
  `docs/analysis/target-dir-disk-reduction/spikes/`:
  - **0.1 lock under load (Windows)** — external std file locks on
    `.cargo-build-lock` block `cargo build`/`check` exactly as designed;
    `try_lock` returns `WouldBlock` during live builds; rust-analyzer holds no
    locks at rest.
  - **0.2 Unix lock/unlink probes (Linux, containerized)** — GATE PASSES:
    flock exclusion works; unlinking artifacts degrades to a surgical rebuild,
    never corruption.
  - **0.3 fingerprint liveness** — "hash absent from artifacts" is NOT a safe
    deletion predicate (those are the live fingerprints of MSVC plain
    binaries); safe rule is pairwise deletion + keep-newest within the orphan
    class; check-mode and build-mode fingerprints are distinct live identities.
  - **0.4 incremental identity** — grammar validated on 10,227 real dirs with
    zero violations; `build_script_build` dirs belong to different packages
    and must never be grouped; dir mtime is the newest-selection key (334/334).
  - **0.5 layout detection** — empirical grammar for the three layouts
    (legacy unified, `build.build-dir` split, nightly layout v2);
    `cargo metadata` exposes `build_directory`; config discovery is CWD-based.
  - **0.6 scheduler registration** — per-user Task Scheduler registration
    works non-elevated via the COM API (daily + only-if-idle); systemd user
    timer and launchd recipes; opportunistic fallback when no scheduler is
    usable.

### Added — engine (Phases 1–2)

- Cargo workspace: `targone-core` (library) + `cargo-targone` (CLI), stable
  toolchain, zero-warning gate (`clippy::all`, `unsafe_code` denied outside
  the two audited syscall shims), 40 unit tests.
- **Discovery** (`discover`): composite discriminator (CACHEDIR.TAG signature /
  `.rustc_info.json` / conventional name + profile grammar); correctly rejects
  non-Cargo dirs named `target` holding user data (test fixtures, datasets)
  that name-based tools would destroy; finds cross-compile and `llvm-cov`
  nested profiles; never follows symlinks or reparse points.
- **Layout probe** (`layout`): fail-closed classification into legacy /
  build-dir-split / layout-v2 / artifact-only / unknown — unknown is never
  swept.
- **Classification** (`scan`): metadata-only (never opens artifact contents —
  preserves the atime signal); identity-recency keyed on
  `(package, unit-state-file set, artifact class)` so check-mode and
  build-mode generations never supersede each other; `build_script_build`
  incremental exception; fail-open — anything unrecognized is kept and
  reported; emits concrete `ReclaimItem` plans (fingerprint dir before its
  artifacts) with rustc session locks attached.
- **Re-collection rule**: hashed artifacts whose fingerprint no longer exists
  (Cargo invariant A\F = 0) are reclaimed on the next run — interrupted
  sweeps are self-healing.
- **Lock protocol** (`lock`): exclusive `.cargo-build-lock` + shared
  `.cargo-lock` (pre-1.96 interlock) via std file locks, byte-compatible with
  Cargo; try-lock-and-skip — never waits, never proceeds unlocked; rustc
  incremental session-lock probing.
- **Sweep executor** (`sweep`): network-filesystem refusal; layout
  re-validation under the held lock; ordered deletion; Windows-hardened
  retries (~2 s window for Defender scan-on-delete holds) with residue
  tolerated, logged, and re-collected next run; append-only JSONL audit log
  at `$CARGO_HOME/targone/audit.jsonl`.
- **CLI**: `cargo targone report [PATHS…] [--json]` and
  `cargo targone gc [PATHS…] [--apply] [--tier …] [--json]` — dry-run is the
  default; only `--apply` deletes.

### Verified

- Recovery gate: build → `gc --apply` → build = **zero recompilations**,
  3/3 rounds, convergent (third round reclaims 0 bytes).
- Concurrency gate (miniature): 30/30 `cargo build`/`check` successes against
  a continuous `gc --apply` loop — all 34 sweep attempts correctly
  lock-skipped.
- Interrupt gate: sweeper killed 120 ms into a live deletion; the target dir
  remained buildable and converged fresh.
- Interrupt gate: sweeper killed 120 ms into a live deletion; the target dir
  remained buildable and converged fresh.
- **Full real-machine sweep (audited): ~252.5 GiB reclaimed** across 16
  target dirs in three passes — Cortex alone went 172.0 → 19.1 GiB (−89%);
  a live build on ar-v3-dashboard was correctly lock-skipped on pass one and
  reclaimed 36.5 GiB on the next; Defender-held residue self-healed via the
  re-collection rule. The one post-sweep `cargo check` failure investigated
  traced to uncommitted work-in-progress source in the swept project
  (name-resolution error — impossible to cause by artifact deletion).
- Read-only `report` on the reference machine: 303.6 GiB total,
  248.5 GiB reclaimable across 16 directories in ~20 s — deliberately on the
  conservative side of the analysis's 257.7 GB Policy A.

### Added — recurrence & adoption (Phases 3–4)

- Machine registry (`$CARGO_HOME/targone/registry.jsonl`, append-only JSONL,
  read-time compaction, orphan detection) and machine config
  (`config.toml`: scan roots, optional global budget).
- Budget engine: ordering-and-stopping over reclaimable bytes only;
  unreachable budgets reported, never implied met.
- `cargo targone scan` — adopts projects into the registry (16 adopted on
  the reference machine); `cargo targone schedule install|uninstall|status|
  run` — per-user Task Scheduler registration, non-elevated, daily 03:00 +
  only-if-idle + battery-friendly + missed-run catch-up; systemd user timer
  and launchd recipes; `TARGONE_DISABLE=1`/CI are hard no-ops; last-run
  summary persisted for `status`. Install/uninstall verified idempotent.
- **`targone` beacon crate**: zero dependencies; its build script appends one
  registry line and exits — never deletes, never spawns, no network, no
  rerun-if directives; hard no-op under `DOCS_RS`/`TARGONE_DISABLE`/`CI`;
  engine-missing hint at most once per day. Live-verified: its real
  registration parsed by the engine. (Deviation from the phase-4 proposal:
  the beacon does not touch the scheduler — that would violate the
  no-spawned-processes invariant.)

[Unreleased]: https://github.com/hivellm/targone/commits/main
