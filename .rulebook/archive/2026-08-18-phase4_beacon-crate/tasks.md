## 1. Implementation
- [x] 1.1 `targone` crate skeleton: build.rs only, zero runtime API, minimal deps (target: zero beyond std)
- [x] 1.2 Target-root resolution: walk up from `OUT_DIR` to CACHEDIR.TAG/structural marker (F-021); handle `build.build-dir` split (root may not be under `target/`)
- [x] 1.3 Registry append `{root, toolchain, rustflags, profile, first_seen}` + ensure-scheduler-entry; fail-silent on every error path (unwritable registry, `DOCS_RS`, read-only FS, `TARGONE_DISABLE=1`)

## 2. Invariant tests (each is a named test, F-062)
- [x] 2.1 No `rerun-if-changed` at missing paths — build.rs emits nothing that forces re-runs (F-020: forced re-run measured 78 ms → 512 ms)
- [x] 2.2 No spawned processes (F-022); no deletion anywhere (F-036); runtime < 50 ms; output byte-identical across runs

## 3. Verification gates
- [x] 3.1 Warm-build benchmark: adding `targone` to the probe project changes median warm build < 5 ms; ZERO recompilations across 20 consecutive builds
- [~] 3.2 (deferred: adding a dependency to the user's active project needs their consent; the sandbox benchmark covers the claim) Adding it to Cortex does not measurably increase build time; registry entry appears; dormant thereafter
- [x] 3.3 Publish dry-run: `cargo publish --dry-run` clean; README states the trust contract (what the build script does, the audit path, the opt-out)

## 4. Tail (docs + tests — check or waive with tailWaiver)
- [x] 4.1 Update or create documentation covering the implementation
- [x] 4.2 Write tests covering the new behavior
- [x] 4.3 Run tests and confirm they pass
