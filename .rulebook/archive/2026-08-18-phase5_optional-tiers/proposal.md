# Proposal: phase5_optional-tiers

> Materializes Phase 5 of docs/analysis/target-dir-disk-reduction/08-execution-plan.md.
> Each item independently switchable, each OFF by default. Polish — if this
> phase never ships, nothing measured in the analysis is lost.
> Depends on: phase3 (config + scheduled runs exist to host the toggles).

## Why
The core plan reclaims 85.7% (Policy A). The remaining measured levers are
either platform-specific (PDB drop: +13.0 GB, Windows only), riskier
(uninstalled-toolchain sweep carries the hash-algorithm maintenance burden,
F-032), more invasive (PATH shim trigger, F-040), or advisory (profile
config hints, F-070). They belong behind explicit opt-in flags, after the
default path has earned trust.

## What Changes
- Tier 5 — drop all PDBs (`--pdbs`): 300.8 → 30.1 GB projected (10.0×);
  costs symbolization of already-built binaries; Windows-relevant only (F-042).
- Tier 6 — dormant directories (`--dormant <days>`): target dirs unbuilt for
  > N days → full reclaim (F-043's one legitimate use of absolute age:
  relative to the dir's own last build, not per-artifact).
- Tier 7 — uninstalled toolchains: cargo-sweep's dual-hash keep-set
  (`rustc-stable-hash` ≥1.85 + legacy SipHasher, plus literal 0) with
  fail-open discipline (F-032, F-046); maintenance burden accepted knowingly.
- PATH shim trigger (opt-in, clearly labeled higher blast radius): post-build
  cleanup with ideal timing (F-040).
- Advice output in `report`: quantified savings from
  `[profile.dev.package."*"] debug = 0`, `-Zno-embed-metadata` (F-070);
  copy-paste config, never auto-edit (F-062).

## Impact
- Affected specs: rust.md
- Affected code: `targone-core` (tiers 5–7), `cargo-targone` (flags, advice)
- Breaking change: NO
- Dependencies: phase3; independent of phase4 (may run in parallel with it)
- User benefit: up to 10× total reduction for users who opt in; actionable
  config advice quantified against their real directories.
