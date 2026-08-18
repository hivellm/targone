# 🧹 Targone

[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-stable%20(1.89%2B)-orange.svg)](https://www.rust-lang.org/)
[![Status](https://img.shields.io/badge/status-phase%200%20(analysis%20complete)-yellow.svg)](#-project-status)
[![Analysis](https://img.shields.io/badge/analysis-8%20documents-blue.svg)](docs/analysis/target-dir-gc/00-README.md)

> **`target/`, gone. Automatic, safe garbage collection for Rust build directories — across every project on your machine.**

Every Rust project's `target/` directory grows forever: Cargo never garbage-collects it, every
toolchain update and dependency bump orphans the previous artifact set, and on a machine with many
projects the waste compounds to **terabytes**. The only stock answer is `cargo clean` — manual,
per-project, all-or-nothing, and it throws away your warm builds.

Targone bounds that growth automatically: add one crate to each project, install one engine on the
machine, and the problem manages itself — selectively, under a disk budget, without ever racing a
live build.

Part of the [HiveLLM](https://github.com/hivellm) family (Nexus · Fluxum · Synap · Vectorizer).

---

## 🎯 Overview

```
Without Targone                        With Targone
─────────────────────                  ──────────────────────────────────────
proj-a/target/   180 GB  ← stale       targone (dep, per project)
proj-b/target/   240 GB  ← stale         │ build.rs: registers project +
proj-c/target/    95 GB  ← deleted       │ activity ping (~1ms, never deletes)
…                                        ▼
  Σ = TBs on the SSD                   ~/.targone/registry
                                         │
manual `cargo clean` ×N                  ▼  scheduled (Task Scheduler /
  = cold rebuilds, forgotten           cargo-targone (engine, per machine)
    projects keep their GBs              • takes Cargo's real build locks
                                         • tiered GC under a disk budget
                                         • orphan & idle project reclaim
                                       Σ stays bounded, warm builds stay warm
```

**Two parts, one product:**

- **`targone`** — the crates.io module you add to each project. Its build script does exactly one
  thing: append the project + timestamp to a local machine registry (~1ms, fail-silent, no
  network, no deletion, auditable in five minutes). It is the adoption interface and the activity
  signal — never the executioner.
- **`cargo-targone`** — the engine, installed once (`cargo install cargo-targone`). Runs on a
  schedule, takes **Cargo's own file locks** (`.cargo-build-lock` — no other tool in the ecosystem
  does this) so it can never corrupt a concurrent build, and applies tiered policies until your
  disk budget is met.

## ✨ Features (per the accepted design — see [analysis](docs/analysis/target-dir-gc/00-README.md))

### 🔒 Safe by construction
- **Lock-honest** — byte-compatible with Cargo 1.96+'s lock protocol via `std` file locks; a
  running build blocks Targone, never the reverse (`try_lock` → skip, never wait, never race)
- **Rebuild-worst-case** — deletion ordering (fingerprints before artifacts) guarantees the worst
  possible outcome of any GC action is a rebuild, never a corrupted or silently-wrong build
- **Windows first-class** — share-mode deletes, rename-then-delete fallback, retry-with-backoff,
  mmap/AV tolerance; primary dev platform, tested in CI
- **Dry-run and journaled** — `--dry-run` default-on in the first release; every pass journaled

### 🧠 Smart, not destructive
- **Tiered policies under a disk budget** — incremental-cache pruning → stale-toolchain sweep
  (every rustup update silently doubles your target dirs) → build-driven mark & sweep →
  idle-project wipe → orphaned-dir reclaim, escalating only as the budget demands
- **No dead signals** — no atime/mtime heuristics (the 8-year-old failure mode of prior tools);
  decisions key on fingerprint toolchain hashes, Cargo's `--message-format=json` artifact
  live-sets, and the activity registry
- **Dual-layout aware** — supports both the legacy `target/` layout and Cargo's build-dir
  layout v2 (default from ~1.99)

### 📉 The horizontal problem too
- **Central build-dir migration (opt-in)** — one command moves ~90% of every project's bytes into
  a single GC-able root via stable `build.build-dir` (measured upstream: 4.2 GB → 415 MB for
  cargo itself), with per-workspace isolation — none of the shared-`CARGO_TARGET_DIR` correctness
  bugs
- **Whole-machine visibility** — `cargo targone status`: per-project size, idleness, and what the
  next pass reclaims; `scan` finds forgotten projects and orphaned target dirs

## 🚀 Planned CLI

```bash
cargo install cargo-targone
cargo targone setup          # scheduled task + machine config (+ --central-build-dir)
cargo targone status         # who is eating the SSD, and what a pass would reclaim
cargo targone gc --dry-run   # one pass, tiered, honest
cargo targone scan E:\code   # adopt unregistered / orphaned target dirs
```

```toml
# per project — the whole integration
[build-dependencies]
targone = "0.1"
```

## 📊 Project Status

| Phase | Scope | Status |
|---|---|---|
| Phase 0 — problem statement + full solution-space analysis | ✅ **Done** — [8 documents](docs/analysis/target-dir-gc/00-README.md) |
| Phase 1 — engine core: locking, dual-layout probe, incremental + toolchain tiers, orphan reclaim, `status`/`scan` | ⏳ Next |
| Phase 2 — set-and-forget: schedulers, size budget, idleness tiers | ⏳ |
| Phase 3 — `targone` registration crate (crates.io module) | ⏳ |
| Phase 4 — mark & sweep precision tier, central build-dir migration | ⏳ |

## 📚 Documentation

- [Problem statement](docs/problem-statement.md) — why this project exists
- [Analysis index](docs/analysis/target-dir-gc/00-README.md) — findings in one page + verdict
  - [01 — Why `target/` grows without bound](docs/analysis/target-dir-gc/01-problem-mechanics.md)
  - [02 — Prior art (cargo-sweep, kondo, sccache, …)](docs/analysis/target-dir-gc/02-prior-art.md)
  - [03 — Cargo's own GC roadmap and timeline](docs/analysis/target-dir-gc/03-cargo-roadmap.md)
  - [04 — Integration mechanisms, with verdicts](docs/analysis/target-dir-gc/04-integration-mechanisms.md)
  - [05 — Locking and safe deletion](docs/analysis/target-dir-gc/05-locking-and-safety.md)
  - [06 — Cleanup policy catalog](docs/analysis/target-dir-gc/06-cleanup-policies.md)
  - [07 — Recommended architecture](docs/analysis/target-dir-gc/07-recommended-architecture.md)

## 📄 License

Licensed under the [Apache License 2.0](LICENSE).

## 🤝 Contributing

This project follows the HiveLLM family conventions: spec-driven development, Conventional
Commits, Keep a Changelog, and zero-warning quality gates.
