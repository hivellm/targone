# Proposal: phase3_scheduler-recurrence

> Materializes Phase 3 of docs/analysis/target-dir-disk-reduction/08-execution-plan.md.
> Turns a tool someone must remember into one that runs itself — the actual
> gap identified in F-010 (cargo-sweep was installed on this machine and never
> run).
> Depends on: phase2 (a safe `gc --apply`), phase0 spike 0.6 (scheduler rights).

## Why
F-010 is the project's founding observation: the correct tool existed,
installed, and was never invoked. A cargo subcommand alone reintroduces that
exact failure (F-025). Only OS-level scheduling gives recurring execution
without touching builds (F-028): no daemon, no shim, no build-time cost.

## What Changes
- `cargo targone schedule install|status|uninstall`: idempotent registration
  with Task Scheduler (Windows), systemd user timer (Linux), launchd (macOS);
  idle-triggered, daily cadence; graceful degradation without rights
  (spike 0.6).
- Registry: `$CARGO_HOME/targone/registry.jsonl` (append-only, durable) +
  configurable scan roots in `$CARGO_HOME/targone/config.toml`; discovery
  reads both (F-059).
- Global budget as an ordering-and-stopping function over RECLAIMABLE bytes
  only (F-048, F-034 — never repeat cargo-sweep's budget-over-undeletable
  mistake); directories processed descending by reclaimable size (F-001).
- Hard no-op switches: `TARGONE_DISABLE=1`, CI detection (F-062.10).

## Impact
- Affected specs: rust.md
- Affected code: `cargo-targone` (schedule + scheduled-run entry point),
  `targone-core` (registry, budget ordering)
- Breaking change: NO
- Dependencies: phase2, phase0 (0.6); blocks phase4
- User benefit: set-and-forget — aggregate target/ usage stays under budget
  with zero human memory required; dormant projects finally get swept.
