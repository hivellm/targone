## 1. Workspace bootstrap
- [x] 1.1 Cargo workspace: `targone-core` (lib) + `cargo-targone` (bin); stable toolchain (1.89+ for std file locks); CI skeleton Windows + Linux; quality gate (type-check → clippy -D warnings → tests)

## 2. targone-core — discovery & layout
- [x] 2.1 Target-dir discovery with composite discriminator (F-055): CACHEDIR.TAG signature OR (`.rustc_info.json` at root + profile dir with `.fingerprint/`); reject the 30 known false-positive `Target/` shapes (LLVM/GCC test set from F-055)
- [ ] 2.2 Reparse-point/symlink skipping; filesystem-type detection (NTFS/ext/APFS vs network FS → refuse, F-061)
- [x] 2.3 Layout model behind one module (F-041 + spike 0.5): legacy layout + `build.build-dir` split + `-Zbuild-dir-new-layout` probe; unrecognized layout → classify nothing, report "unknown layout"
- [x] 2.4 Metadata-only enumeration (F-056): sizes/mtimes/atimes without opening file contents; test asserts atime unchanged after full scan

## 3. targone-core — classification (F-049 tiers, read-only)
- [x] 3.1 Tier 1: incremental keep-newest-N-per-crate grouping (spike 0.4 parsing rules)
- [x] 3.2 Tier 2: deps hash extraction (16-hex suffix) + newest-identity grouping via `.fingerprint` join (F-005)
- [x] 3.3 Tier 3: build/ keep-newest-per-package grouping
- [x] 3.4 Tier 4: orphan fingerprint detection (rule from spike 0.3)
- [x] 3.5 Fail-open invariant as a type: anything not positively classified as superseded is `Keep` — asserted in tests (F-060)

## 4. cargo-targone report
- [x] 4.1 `cargo targone report [PATHS…]`: per-dir totals, per-pool breakdown, reclaimable per tier, projected residual; `--json` output
- [ ] 4.2 Acceptance: reproduce the machine's measured aggregate within ±2% (total 300.8 GB, incremental 201.8, deps 88.4, build 3.2, Policy A 257.7 GB reclaimable); finds all 18 target dirs, zero false positives on the LLVM/GCC set

## 5. Tail (docs + tests — check or waive with tailWaiver)
- [ ] 5.1 Update or create documentation covering the implementation
- [ ] 5.2 Write tests covering the new behavior
- [ ] 5.3 Run tests and confirm they pass
