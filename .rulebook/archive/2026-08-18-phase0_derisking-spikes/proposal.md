# Proposal: phase0_derisking-spikes

> Materializes Phase 0 of docs/analysis/target-dir-disk-reduction/08-execution-plan.md.
> Answers the open questions in F-064 that gate design decisions. Timeboxed;
> each spike produces a **written result** (docs/analysis/target-dir-disk-reduction/spikes/),
> not a feature. No product code.

## Why
Three design decisions in the architecture (F-058, F-061) rest on facts verified
only on Windows or only from source reading: whether an external `flock` actually
excludes Cargo on Unix, whether deleting open artifacts corrupts builds on
Linux/macOS, and whether `.fingerprint/` entries can be deleted safely
(cargo-sweep says yes, cargo-mark-sweep says no — F-064.4). Building Phase 2 on
an unverified assumption risks the project's core promise (never corrupt a
build).

## What Changes
Six written spike results, no production code:
- 0.1 lock-under-load: hold `.cargo-build-lock` 1/10/60s while rust-analyzer
  runs in 3 workspaces; find the largest sweep unit that stays imperceptible
- 0.2 Unix lock + unlink probes: repeat F-050/F-053 on Linux (container ok)
  and macOS if available — is flock exclusion effective; does unlink of an
  open artifact corrupt a build
- 0.3 `.fingerprint` liveness: resolve the cargo-sweep/cargo-mark-sweep
  contradiction (measured 3,569 fingerprint dirs vs 3,371 deps hashes)
- 0.4 incremental identity parsing: is `name-<disambiguator>` grouping robust
  (crate names containing `-`, non-Cargo dirs)
- 0.5 layout detection: minimum assumption set; detect `build.build-dir` /
  `-Zbuild-dir-new-layout`; fail-closed rule
- 0.6 scheduler registration: Task Scheduler / systemd user timer / launchd —
  rights needed, idempotent registration, behavior without rights

## Impact
- Affected specs: none yet (results feed the phase 1–3 specs)
- Affected code: none (spike scripts live in scratchpad or spikes/ dir)
- Breaking change: NO
- Dependencies: none — this is the entry point; 0.6 may run in parallel with phase 1–2
- User benefit: Phase 2 (the 257.7 GB reclaim) can be built on verified ground.
  If 0.2 shows flock cannot exclude Cargo on Unix, the sweep protocol (F-061)
  is redesigned BEFORE any deletion code exists — stop-and-rethink gate.
