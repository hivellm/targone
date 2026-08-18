# targone-core

The engine library behind [`cargo-targone`](https://crates.io/crates/cargo-targone)
— **`target/`, gone**: automatic, safe garbage collection for Rust build
directories.

What it provides:

- **Discovery** of target/build directories under scan roots, with a composite
  discriminator that refuses non-Cargo directories merely named `target`
  (deleting those would destroy user data).
- **Layout probing** for all three on-disk layouts (legacy unified,
  `build.build-dir` split, and build-dir layout v2), fail-closed: an
  unrecognized layout is never swept.
- **Metadata-only classification** by *identity-recency* — keep the newest
  generation per `(package, unit-state-file set, artifact class)` — so
  check-mode and build-mode artifacts never supersede each other and anything
  unrecognized is kept (fail-open). Produces concrete, ordered deletion plans.
- **Cargo's lock protocol** via plain std file locks (`.cargo-build-lock`
  exclusive + `.cargo-lock` shared): a running build blocks the sweep, never
  the reverse; network filesystems are refused.
- **The sweep executor**: fingerprint-before-artifacts ordering (worst case of
  any sweep is a rebuild, never corruption), Windows-hardened retries with
  residue tolerated and re-collected on the next run, JSONL audit records.
- Registry, byte-budget selection, and the opt-in PDB/dormant tiers.

This crate is the reusable core; most users want the
[`cargo targone`](https://crates.io/crates/cargo-targone) CLI and the
[`targone`](https://crates.io/crates/targone) beacon instead. Full design
notes and measured results live in the
[repository](https://github.com/hivellm/targone).

## License

Apache-2.0
