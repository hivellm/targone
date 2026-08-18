# Targone — Problem Statement

> Initial description of the problem this project exists to solve.
> Date: 2026-08-18 · Status: draft (pre-analysis)

## The problem

Every Rust project keeps its own `target/` directory. It accumulates build
artifacts continuously and Cargo never garbage-collects it:

- **Per-profile duplication** — `debug/`, `release/`, and any custom profiles
  each keep a full copy of every compiled dependency.
- **Stale artifacts pile up** — every dependency version bump, feature-flag
  change, `RUSTFLAGS` change, or toolchain update produces a *new* set of
  artifacts next to the old ones. The old ones are never removed.
- **Incremental compilation cache** (`target/*/incremental/`) grows without
  bound and is frequently invalidated but never pruned.
- **No cross-project sharing** — ten projects using the same `tokio` version
  compile and store it ten times.

On a machine with many active Rust projects this compounds to hundreds of GB
per project in the worst cases and **terabytes in aggregate**, filling SSDs.

## Current (bad) workaround

Manually running `cargo clean` in each project, or deleting `target/`
directories by hand. Problems with this:

1. **Manual and forgettable** — has to be remembered, per project.
2. **All-or-nothing** — `cargo clean` deletes *everything*, so the next build
   is a full cold rebuild (minutes of wasted compile time), even though most
   deleted artifacts were still useful.
3. **Doesn't scale** — with dozens of projects, hunting down every `target/`
   is itself a chore; forgotten/archived projects keep their GBs forever.

## What Cargo itself offers today

- `cargo clean` — full wipe only (or `-p <crate>`, rarely useful for this).
- Automatic garbage collection exists for the **global** cache
  (`$CARGO_HOME`: registry sources, crate tarballs) in recent Cargo versions —
  but **not** for per-project `target/` directories, which is where the bulk
  of the space goes. Target-dir GC has been an open Cargo issue for years.

## Goal

A **Rust crate (crates.io module) added as a dependency to each project** that
takes care of this problem automatically — reducing `target/` disk usage as
much as possible, with minimal ceremony:

- Added once per project; from then on the problem manages itself.
- Smart, not destructive: prefer pruning stale/unused artifacts over full
  wipes, so warm builds stay warm.
- Should address both dimensions of the problem:
  - **vertical** — one project's `target/` growing without bound;
  - **horizontal** — many projects each holding redundant copies of the same
    compiled dependencies.

## Constraints & open questions (input for the analysis)

- **Delivery mechanism**: what can a *dependency crate* actually do? Cargo has
  no post-build hooks; a crate's `build.rs` runs at build time — is that the
  right (or only) integration point? Does it need a companion cargo
  subcommand or background helper? The analysis must map every viable
  mechanism and its trade-offs.
- **Safety**: never delete artifacts of the *current* build; never corrupt a
  concurrent build (Cargo file locks); Windows + Unix support (primary dev
  machine is Windows).
- **Policy**: what counts as "stale"? age-based, toolchain-based,
  last-access-based, size-budget-based?
- **Prior art to evaluate**: `cargo-sweep`, `kondo`, `cargo-cache`, shared
  `CARGO_TARGET_DIR`, sccache, Cargo's own GC roadmap.

## Success criteria (draft)

1. Aggregate `target/` disk usage across all projects drops by an order of
   magnitude and *stays* bounded.
2. No manual per-project intervention after initial adoption.
3. Build-time cost of the mechanism is negligible; warm builds are preserved
   where possible.
4. Safe by default — cannot break or corrupt an in-progress build.
