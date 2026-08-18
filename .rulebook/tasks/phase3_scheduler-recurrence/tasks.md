## 1. Registry & config
- [x] 1.1 Registry module: append-only JSONL at `$CARGO_HOME/targone/registry.jsonl` (schema-versioned entries: root, target dirs, toolchain, first/last seen); compaction on read by the engine, never by writers
- [x] 1.2 Machine config `$CARGO_HOME/targone/config.toml`: scan roots, global budget, tier toggles, cadence
- [x] 1.3 Discovery reads registry + scan roots (F-059); `cargo targone scan <roots>` adopts unregistered dirs and records orphans (workspace manifest gone → full-reclaim candidates)

## 2. Budget engine
- [x] 2.1 Global budget as ordering-and-stopping over reclaimable bytes only (F-048): rank dirs descending by reclaimable size (F-001 heavy tail), sweep until under budget, stop
- [x] 2.2 Budget measured only over deletable pools — regression test encoding cargo-sweep's F-034 mistake as a must-not

## 3. Scheduler integration
- [ ] 3.1 `cargo targone schedule install|status|uninstall`: Task Scheduler (Windows) idempotent registration, idle-triggered daily; exact rights model per spike 0.6
- [x] 3.2 systemd user timer (Linux) + launchd (macOS) equivalents
- [x] 3.3 Scheduled entry point: silent, budget-driven `gc --apply` with audit log; `TARGONE_DISABLE=1` and CI detection → hard no-op
- [ ] 3.4 Uninstall leaves no scheduler entry, no daemon, no stray files

## 4. Verification gates
- [ ] 4.1 Survives reboot; runs unattended for a week on the reference machine; aggregate stays under budget
- [ ] 4.2 A dormant project's directory is swept without any project interaction (registry durability, F-017)

## 5. Tail (docs + tests — check or waive with tailWaiver)
- [ ] 5.1 Update or create documentation covering the implementation
- [ ] 5.2 Write tests covering the new behavior
- [ ] 5.3 Run tests and confirm they pass
