# Analysis: Rust `target/` garbage collection — possibilities for Targone

> Full analysis of the solution space for automatically bounding `target/` disk
> usage across many Rust projects. Research date: **2026-08-18**, baseline
> **Cargo/Rust 1.97.0 stable** (1.98 ~2026-08-20). Sources: Cargo source at
> current stable, GitHub issue tracker, crates.io metadata, tool source code.

## Files

| # | File | Theme |
|---|------|-------|
| 01 | [01-problem-mechanics.md](01-problem-mechanics.md) | Why `target/` grows without bound — layout anatomy, hashes, what never gets deleted |
| 02 | [02-prior-art.md](02-prior-art.md) | Every existing tool, what it actually does, and why none solves this |
| 03 | [03-cargo-roadmap.md](03-cargo-roadmap.md) | What Cargo itself ships, has accepted, and is building — and the timeline that constrains us |
| 04 | [04-integration-mechanisms.md](04-integration-mechanisms.md) | Every way a crates.io module can hook into a project — with verdicts |
| 05 | [05-locking-and-safety.md](05-locking-and-safety.md) | Cargo's lock protocol, safe deletion order, Windows hazards |
| 06 | [06-cleanup-policies.md](06-cleanup-policies.md) | What to delete and how to decide — policy catalog with verdicts |
| 07 | [07-recommended-architecture.md](07-recommended-architecture.md) | The recommended Targone design |

## Findings in one page

1. **Nothing on crates.io does this.** No published crate prunes `target/` as a
   project dependency — verified by zero reverse-deps on every candidate, zero
   GitHub `build.rs` code hits, and no library target on `cargo-sweep`. The
   niche is empty for structural reasons, not for lack of demand (§02, §04).
2. **Cargo won't solve it soon.** Automatic GC (stable since 1.88) covers
   `$CARGO_HOME` only. Whole-`target/` GC is accepted (#13136) but unbuilt;
   intra-`target/` GC is blocked on a layout rewrite that only reaches stable
   ~Oct 2026 — and that rewrite changes the directory structure we must
   support (§03).
3. **`build.rs` cannot be the GC engine.** It has no target-dir path (request
   is `S-propose-close`), doesn't run on warm builds, runs at the worst
   possible moments (mid-build, docs.rs, `cargo publish`), deadlocks on
   Cargo's own lock, and a file-deleting build script is behaviorally
   indistinguishable from the 2023 crates.io malware pattern (§04).
   **It can, however, safely *register* the project** — a ~1ms append to a
   machine-global registry — which is exactly the signal a GC engine needs.
4. **Safe pruning is possible and nobody does it.** Cargo 1.96+ exposes a
   byte-compatible lock protocol (`.cargo-build-lock`, plain `std` file
   locks); taking it exclusively makes concurrent builds block politely.
   No existing tool takes any lock (§05).
5. **atime/mtime-based policies are dead.** atime is off by default on
   Windows and `relatime` on Linux; Cargo deliberately backdates fingerprint
   mtimes. This is the root cause of cargo-sweep's 8-year-old open bug #11.
   Sound signals are: the fingerprint `rustc` hash (toolchain sweep), the
   `--message-format=json` artifact live-set (mark & sweep), and an own
   registry of build activity (§06).
6. **The biggest single lever is configuration, not deletion:**
   `build.build-dir` (stable 1.91) relocates ~90% of bytes out of per-project
   `target/` into one central, GC-able location — measured 4.2 GB → 415 MB on
   cargo itself (§03, §07).

## Verdict

Targone should be a **two-part system** (§07):

- **`targone`** — the per-project dependency crate the user adds once. It
  never deletes anything; it registers the project + build activity in a
  machine-global registry and carries per-project policy config.
- **`cargo-targone`** — the engine (cargo subcommand + optional scheduled
  task). Reads the registry, takes Cargo's real locks, and applies tiered
  policies: incremental-cache pruning → stale-toolchain sweep → mark & sweep
  → size budget → idle-project wipe. Optionally migrates projects to a
  central `build.build-dir`.
