# Proposal: phase2_sweep-under-lock

> Materializes Phase 2 of docs/analysis/target-dir-disk-reduction/08-execution-plan.md.
> The phase that reclaims the measured 257.7 GB (85.7%). The project's entire
> reputational risk sits here (F-051, F-057) — the concurrency test IS the
> deliverable.
> Depends on: phase1 (classification), phase0 spikes 0.1–0.3 (lock behavior,
> Unix probes, fingerprint liveness).

## Why
Deletion is the product. Every prior tool either deletes everything (kondo,
cargo-clean-all) or deletes unsafely without locks (cargo-sweep corrupts
concurrent builds, F-035). The differentiator is the sweep protocol (F-061):
verified target, refused network FS, Cargo's own `.cargo-build-lock` held
exclusively per profile dir, identity-recency classification, per-file
deletion tolerant of Windows sharing violations, and a worst case of
"one rebuild", never corruption.

## What Changes
- Sweep protocol in `targone-core` exactly as F-061: verify → refuse network
  FS → acquire `.cargo-build-lock` (std file locks; `.cargo-lock` shared for
  pre-1.96 interlock) → classify → delete → release; one profile dir per
  acquisition; try_lock-and-skip, never wait, never proceed unlocked.
- Policy tiers 1–4 (F-049): incremental keep-newest-1-per-crate; deps
  newest-hash-only; build keep-newest-per-package; orphan fingerprints only
  if spike 0.3 cleared it.
- `cargo targone gc`: `--dry-run` is the DEFAULT; `--apply` required to
  delete; append-only audit log (path, reason, size, timestamp) per run.
- Windows-hardened deletion (F-037, F-053): `remove_dir_all` crate,
  share-violation tolerance with retry/backoff, residue tolerated and logged.

## Impact
- Affected specs: quality.md (concurrency test gates), rust.md
- Affected code: `targone-core` (sweep module), `cargo-targone` (`gc` command)
- Breaking change: NO
- Dependencies: phase0 (0.1–0.3), phase1; blocks phase3
- User benefit: 300.8 GB → ≤45 GB on the reference machine with zero cold
  rebuilds; worst case of any sweep is a re-link or one slower compile.
