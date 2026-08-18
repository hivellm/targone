## 1. Implementation
- [ ] 1.1 Spike 0.1 — lock under load: hold `.cargo-build-lock` for 1/10/60 s while rust-analyzer + cargo check run in 3 workspaces; record largest imperceptible sweep unit → `spikes/01-lock-under-load.md`
- [ ] 1.2 Spike 0.2 — Unix probes: on Linux (container acceptable), verify external flock blocks `cargo build` with Cargo's own "Blocking waiting for file lock" message; verify unlink-while-open degrades to rebuild, not corruption → `spikes/02-unix-lock-unlink.md` (STOP-AND-RETHINK gate for phase 2 if negative)
- [ ] 1.3 Spike 0.3 — `.fingerprint` liveness: enumerate fingerprint dirs whose hash appears in no `deps/`/`build/` filename on 3 real projects; classify them (run-build-script units?); decide delete/keep rule → `spikes/03-fingerprint-liveness.md`
- [ ] 1.4 Spike 0.4 — incremental identity parsing: validate `name-<disambiguator>` grouping against crate names with `-`, and confirm non-Cargo dirs are refused → `spikes/04-incremental-identity.md`
- [ ] 1.5 Spike 0.5 — layout detection: define the minimum structural assumption set; probe `build.build-dir` and `-Zbuild-dir-new-layout` trees; write the fail-closed rule → `spikes/05-layout-detection.md`
- [ ] 1.6 Spike 0.6 — scheduler registration (parallel-ok): Task Scheduler / systemd user timer / launchd — rights, idempotency, no-rights behavior → `spikes/06-scheduler-registration.md`

## 2. Tail (docs + tests — check or waive with tailWaiver)
- [ ] 2.1 Update or create documentation covering the implementation
- [ ] 2.2 Write tests covering the new behavior
- [ ] 2.3 Run tests and confirm they pass
