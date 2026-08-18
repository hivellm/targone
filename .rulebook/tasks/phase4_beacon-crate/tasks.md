## 1. Implementation
- [ ] 1.1 `targone` crate skeleton: build.rs only, zero runtime API, minimal deps (target: zero beyond std)
- [ ] 1.2 Target-root resolution: walk up from `OUT_DIR` to CACHEDIR.TAG/structural marker (F-021); handle `build.build-dir` split (root may not be under `target/`)
- [ ] 1.3 Registry append `{root, toolchain, rustflags, profile, first_seen}` + ensure-scheduler-entry; fail-silent on every error path (unwritable registry, `DOCS_RS`, read-only FS, `TARGONE_DISABLE=1`)

## 2. Invariant tests (each is a named test, F-062)
- [ ] 2.1 No `rerun-if-changed` at missing paths — build.rs emits nothing that forces re-runs (F-020: forced re-run measured 78 ms → 512 ms)
- [ ] 2.2 No spawned processes (F-022); no deletion anywhere (F-036); runtime < 50 ms; output byte-identical across runs

## 3. Verification gates
- [ ] 3.1 Warm-build benchmark: adding `targone` to the probe project changes median warm build < 5 ms; ZERO recompilations across 20 consecutive builds
- [ ] 3.2 Adding it to Cortex does not measurably increase build time; registry entry appears; dormant thereafter
- [ ] 3.3 Publish dry-run: `cargo publish --dry-run` clean; README states the trust contract (what the build script does, the audit path, the opt-out)

## 4. Tail (docs + tests — check or waive with tailWaiver)
- [ ] 4.1 Update or create documentation covering the implementation
- [ ] 4.2 Write tests covering the new behavior
- [ ] 4.3 Run tests and confirm they pass
