# Proposal: phase4_beacon-crate

> Materializes Phase 4 of docs/analysis/target-dir-disk-reduction/08-execution-plan.md.
> The `cargo add targone` adoption path. Deliberately LAST: F-059 shows the
> beacon adds a few percent of coverage, and F-062 shows it is the easiest
> component to make actively harmful.
> Depends on: phase3 (registry + scheduler exist for it to feed).

## Why
The product requirement is "add a crate to each project and the problem
manages itself." The safe reading (F-058) is a beacon: the build script
registers the project and ensures a scheduler entry exists — never deletes,
never spawns, never re-runs. It adds the four things a scan cannot get
(F-059): relocated target dirs, toolchain/RUSTFLAGS provenance, enrolment
without configuring scan roots, and a durable record that outlives dormancy.

## What Changes
New crate `targone` (build.rs only, no runtime API):
- walk up from `OUT_DIR` to the CACHEDIR.TAG/target root (F-021), append
  `{root, toolchain, rustflags, profile, first_seen}` to the registry,
  ensure a scheduler entry exists, exit.
- Hard invariants, each with a test (F-020, F-022, F-036, F-062):
  no `rerun-if-changed` pointing at a missing path (would force rebuilds);
  no spawned processes; no deletion; < 50 ms runtime; byte-identical output
  across runs; fail-silent on unwritable registry, `DOCS_RS`, read-only FS,
  `TARGONE_DISABLE=1`.

## Impact
- Affected specs: rust.md
- Affected code: new crate `targone`
- Breaking change: NO
- Dependencies: phase3
- User benefit: one-line adoption per project (`[build-dependencies]
  targone = "0.1"`); trust surface auditable in five minutes — the explicit
  antithesis of the build-script-malware pattern.
