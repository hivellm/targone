## 1. Implementation (each item independently switchable, OFF by default)
- [ ] 1.1 Tier 5 — PDB drop (`--pdbs`, Windows): delete `.pdb` files under swept pools; report symbolization cost honestly; projected 300.8 → 30.1 GB verified on reference machine (F-042)
- [ ] 1.2 Tier 6 — dormant dirs (`--dormant <days>`): age measured against the dir's own newest build, never wall-clock-per-artifact (F-043); full reclaim path with the same audit log
- [ ] 1.3 Tier 7 — uninstalled-toolchain sweep: dual-hash keep-set (rustc-stable-hash + legacy SipHasher + literal 0), fail-open on unreadable fingerprints (F-032, F-046); document the hash-drift maintenance contract
- [ ] 1.4 PATH shim trigger (opt-in): wrap cargo, GC after exit (F-040); install/uninstall commands; labeled higher-blast-radius in docs
- [ ] 1.5 Advice output in `report`: quantify `[profile.dev.package."*"] debug = 0` and `-Zno-embed-metadata` savings on the scanned dirs (F-070); print copy-paste config; never auto-edit manifests

## 2. Tail (docs + tests — check or waive with tailWaiver)
- [ ] 2.1 Update or create documentation covering the implementation
- [ ] 2.2 Write tests covering the new behavior
- [ ] 2.3 Run tests and confirm they pass
