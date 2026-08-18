# Proposal: phase1_core-and-report

> Materializes Phase 1 of docs/analysis/target-dir-disk-reduction/08-execution-plan.md.
> Everything except deletion. Ships as a usable disk-analysis tool.
> Depends on: phase0 spikes 0.4 and 0.5 (incremental identity, layout detection).

## Why
The analysis (F-001…F-049) must become executable and re-runnable before any
deletion code exists: a read-only `report` command carries zero risk, makes
every later phase measurable rather than asserted, and its acceptance test is
reproducing the measured numbers on the reference machine (±2%).

## What Changes
New cargo workspace with two crates:
- `targone-core` (library): target-dir discovery (composite discriminator per
  F-055 — CACHEDIR.TAG alone misses 57% of the bytes; reparse points skipped;
  filesystem-type detection with network-FS refusal), layout model isolated
  behind one module with fail-closed default (F-041, spike 0.5),
  metadata-only enumeration (F-056 — a scan must not update atime),
  classification into the F-049 policy tiers (incremental keep-newest-1,
  deps newest-hash, build newest-per-package, orphan fingerprints).
- `cargo-targone` (binary): `report` command — per target dir: total size,
  per-pool breakdown (incremental/deps/build/fingerprint), reclaimable bytes
  per tier, projected residual.

## Impact
- Affected specs: .rulebook/specs/rust.md (workspace conventions apply)
- Affected code: new crates `targone-core`, `cargo-targone`
- Breaking change: NO
- Dependencies: phase0_derisking-spikes (1.4, 1.5); blocks phase2, phase3
- User benefit: immediate visibility ("who is eating the SSD, what is
  reclaimable per tier") with zero deletion risk; the whole analysis becomes
  re-runnable on any machine.
