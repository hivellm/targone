## 1. Lock protocol
- [ ] 1.1 Lock module: exclusive `try_lock` on `<profile>/.cargo-build-lock` + shared `.cargo-lock` (pre-1.96 interlock) via std file locks; on failure → skip dir this run; NFS/network FS → refuse (spike 0.2 result)
- [ ] 1.2 Verify-before-delete guard: re-validate target-dir markers under the held lock (close the check-then-delete window, F-054)

## 2. Deletion engine
- [ ] 2.1 Per-file deleter with Windows hardening (F-053): share-violation tolerance, retry with backoff, rename-then-delete fallback, residue logged not fatal; `remove_dir_all` crate for directory tiers
- [ ] 2.2 Deletion ordering: fingerprint entry before its artifacts (a missing output = stale = safe rebuild; the reverse is the hazard)
- [ ] 2.3 Append-only audit log: every removed path with reason, tier, size, run id

## 3. Policy tiers wired to the engine (F-049)
- [ ] 3.1 Tier 1 — incremental keep-newest-1-per-crate (96.9% of the largest pool, F-003); respect rustc session-dir flocks
- [ ] 3.2 Tier 2 — deps newest-hash-only via fingerprint join (F-005, F-044)
- [ ] 3.3 Tier 3 — build/ keep-newest-per-package
- [ ] 3.4 Tier 4 — orphan fingerprints (ONLY if spike 0.3 cleared it; otherwise waive with written reason)

## 4. CLI
- [ ] 4.1 `cargo targone gc [PATHS…]` — dry-run DEFAULT, `--apply` to delete, `--tier` filter, human + `--json` summaries (freed per tier, skipped-locked dirs, residue)

## 5. Verification gates (the deliverable)
- [ ] 5.1 Concurrency test: sweep loop vs continuous `cargo build`/`cargo check` on the same target dir, ≥100 iterations, zero build failures, zero corrupt artifacts (Windows + Linux CI)
- [ ] 5.2 Recovery test: after full Policy A sweep, `cargo build` on unchanged tree performs ZERO recompilations (F-042 at project scale)
- [ ] 5.3 Interrupt test: kill sweeper mid-delete; target dir remains buildable
- [ ] 5.4 On the reference machine: 300.8 GB → ≤45 GB and a full `cargo test` on Cortex passes afterwards

## 6. Tail (docs + tests — check or waive with tailWaiver)
- [ ] 6.1 Update or create documentation covering the implementation
- [ ] 6.2 Write tests covering the new behavior
- [ ] 6.3 Run tests and confirm they pass
