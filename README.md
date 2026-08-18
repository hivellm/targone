# 🧹 Targone

[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-stable%20(1.89%2B)-orange.svg)](https://www.rust-lang.org/)
[![Status](https://img.shields.io/badge/status-phase%202%20(sweep%20engine%20live)-green.svg)](#-project-status)
[![Analysis](https://img.shields.io/badge/analysis-70%20findings%20%C2%B7%2010%20documents-blue.svg)](docs/analysis/target-dir-disk-reduction/00-README.md)

> **`target/`, gone. Automatic, safe garbage collection for Rust build directories — across every project on your machine.**

Every Rust project's `target/` directory grows forever: Cargo never garbage-collects it, every
dependency bump and toolchain update orphans the previous artifact set, and on a machine with many
projects the waste compounds to hundreds of GB — measured here: **300.5 GB across 18 target
directories, 91% of it in just 3 projects, 65–74% of it in `incremental/` caches that nothing will
ever read again**. The only stock answer is `cargo clean` — manual, per-project, all-or-nothing,
and it throws away your warm builds. (The correct tool, `cargo-sweep`, was installed on this very
machine — and never run. The gap is not algorithms; it is *automatic invocation*.)

Targone bounds that growth automatically: add one crate to each project, and the problem manages
itself — selectively, under a global disk budget, without ever racing a live build. Measured
target on the reference machine: **300.8 GB → ~43 GB (85.7% reclaimed) with zero cold rebuilds**.

Part of the [HiveLLM](https://github.com/hivellm) family (Nexus · Fluxum · Synap · Vectorizer).

---

## 🎯 Overview

```
Without Targone                          With Targone
─────────────────────                    ─────────────────────────────────────────
Cortex/target      172 GB  ← 74% stale   targone (crates.io dep, per project)
Thunder/target      57 GB  ← 69% stale     │ build.rs beacon: registers the project
dashboard/target    45 GB  ← 67% stale     │ (~once ever, never deletes, never spawns)
…                                          ▼
  Σ = 300 GB and growing                 registry — $CARGO_HOME/targone/registry.jsonl
                                           │
manual `cargo clean` ×N                    ▼
  = cold rebuilds, and forgotten         OS scheduler (Task Scheduler / systemd / launchd)
    projects keep their GBs forever        │ the only source of recurrence
                                           ▼
                                         cargo-targone + targone-core (per machine)
                                           • acquires Cargo's own .cargo-build-lock
                                           • keep-newest-per-identity sweep, per tier
                                           • ordered by reclaimable bytes, global budget
                                         Σ stays bounded, warm builds stay warm
```

**Beacon → registry → scheduler → sweeper.** Each part does the one thing it is structurally
capable of doing safely:

- **`targone`** — the dependency you `cargo add`. Its build script appends the project to a local
  registry and exits (< 50 ms, fail-silent, no network, no deletion, no spawned processes —
  auditable in five minutes). It is the adoption interface, deliberately not load-bearing.
- **`cargo-targone` / `targone-core`** — the engine, installed once. Invoked by the OS scheduler,
  it takes **Cargo's real build locks** before touching a profile directory (no existing tool
  does this), classifies artifacts by identity-recency, and deletes only what a future build can
  provably never use.

## ✨ Design highlights (all measured or source-verified — see the [analysis](docs/analysis/target-dir-disk-reduction/00-README.md))

### 🔒 Safe by construction
- **Lock-honest** — byte-compatible with Cargo 1.96+'s `.cargo-build-lock` protocol via std file
  locks; a running build blocks Targone, never the reverse; network filesystems are refused
- **Rebuild-worst-case** — fail-open classification: anything not positively identified as
  superseded is kept; the worst outcome of any sweep is a re-link or one slower compile
- **Windows first-class** — share-violation tolerance, mmap/AV/running-exe awareness,
  metadata-only scanning (a scan must not destroy the atime signal it reads)
- **Dry-run default** — `--apply` required to delete; append-only audit log of every removal

### 🧠 The right policy, proven on real data
- **Identity-recency, not age** — "keep the newest N per identity key" has no cliff: age-based
  rules provably degenerate into `cargo clean` (100% of a warm cache gone at day 8) while never
  touching the bloat of daily-built projects
- **`incremental/` first** — 41× duplication measured (8,113 dirs for 196 crates); keep-newest-1
  reclaims 96.9% of the largest byte pool at zero correctness cost
- **`deps/` surgically** — only `.rlib`/`.rmeta` keep builds warm (4.1 GB of 42.4 GB measured);
  stale test binaries and PDBs are terminal outputs nothing reads back
- **Global budget as trigger, not rule** — directories processed in descending reclaimable-bytes
  order until the machine fits the budget; no uniform per-project quotas

### 🚫 Anti-requirements (things Targone will never do)
- Never delete from inside a build script — structurally unsafe, and upstream has refused the
  hooks that could make it safe for a decade
- Never a shared `CARGO_TARGET_DIR` — measured benefit on this fleet: 5.4%; correctness bugs and
  lock contention: real
- Never a resident daemon, never network access, never auto-edit of your manifests

## 🚀 Planned CLI

```bash
cargo install cargo-targone
cargo targone report                    # who is eating the SSD; reclaimable bytes per tier
cargo targone gc --dry-run              # what a sweep would do (dry-run is the default)
cargo targone gc --apply                # do it, under the lock protocol
cargo targone schedule install          # Task Scheduler / systemd timer / launchd — set & forget
```

```toml
# per project — the whole integration
[build-dependencies]
targone = "0.1"
```

## 📊 Project Status

| Phase | Scope | Status |
|---|---|---|
| Phase 0 — problem statement, measurements, full solution-space analysis (70 findings) | ✅ **Done** — [10 documents](docs/analysis/target-dir-disk-reduction/00-README.md) |
| Phase 0.x — de-risking spikes (lock-under-load, Unix probes, fingerprint liveness) | ✅ **Done** — [6 spikes](docs/analysis/target-dir-disk-reduction/spikes/), all gates passed |
| Phase 1 — `targone-core` + read-only `report` (reproduces the measurements) | ✅ **Done** — acceptance met; discovery correctly rejects the data dirs the original measurement misclassified |
| Phase 2 — deletion under the lock protocol | ✅ **Done** — all gates passed (recovery 0-recompile 3/3; 100-build concurrency gate; interrupt; ~252 GiB reclaimed on the reference machine, +213 GB disk free) |
| Phase 3 — recurrence via OS scheduler + global budget | ✅ Live — daily only-if-idle task installed, 17 projects registered; week-long unattended gate in passive verification |
| Phase 4 — the `targone` beacon crate | ✅ **Done** — zero-dep, invariants tested, +3.1 ms warm-build cost, publish dry-run clean; crates.io publish pending owner decision |
| Phase 5 — opt-in tiers | ✅ **Done** — `gc --pdbs` (23 GiB measured) and `gc --dormant <days>` shipped; toolchain sweep & PATH shim deferred with recorded rationale |

## 📚 Documentation

- [Problem statement](docs/problem-statement.md) — why this project exists
- [Analysis index](docs/analysis/target-dir-disk-reduction/00-README.md) — 70 findings in ten bullets + verdict
  - [01 — Measurements: what is actually on this disk](docs/analysis/target-dir-disk-reduction/01-measurements.md)
  - [02 — Anatomy & growth: where the bytes live](docs/analysis/target-dir-disk-reduction/02-anatomy-and-growth.md)
  - [03 — Integration mechanisms, with experiments](docs/analysis/target-dir-disk-reduction/03-integration-mechanisms.md)
  - [04 — Prior art (cargo-sweep, kondo, sccache, …)](docs/analysis/target-dir-disk-reduction/04-prior-art.md)
  - [05 — Policies, simulated on real data](docs/analysis/target-dir-disk-reduction/05-policies.md)
  - [06 — Safety & concurrency](docs/analysis/target-dir-disk-reduction/06-safety-and-concurrency.md)
  - [07 — Recommended architecture](docs/analysis/target-dir-disk-reduction/07-architecture-recommendation.md)
  - [08 — Execution plan](docs/analysis/target-dir-disk-reduction/08-execution-plan.md)
  - [09 — Cargo upstream roadmap](docs/analysis/target-dir-disk-reduction/09-cargo-upstream-roadmap.md)

## 📄 License

Licensed under the [Apache License 2.0](LICENSE).

## 🤝 Contributing

This project follows the HiveLLM family conventions: spec-driven development, Conventional
Commits, Keep a Changelog, and zero-warning quality gates.
