# 01 — Why `target/` grows without bound

## The core defect, in Cargo's own words

Cargo groups build artifacts by **role**, not by **build unit**:
`.fingerprint/` holds freshness data, `deps/` holds outputs, `build/` holds
build-script state. There is **no index mapping a unit to its files**. The
Cargo team's own diagnosis (Inside Rust blog, cycle 1.92):

> "if we were to GC the content, we'd need to track individual files for a
> build unit" (#5026)

That is why `cargo clean -p` resorts to filename-prefix globbing, why Cargo
has never GC'd `target/`, and why every third-party tool reverse-engineers
the layout. Every artifact that stops being referenced — an old dependency
version, a changed feature set, changed `RUSTFLAGS`, a previous toolchain —
simply stays on disk forever, next to its replacement.

## Layout anatomy (current stable, Cargo 1.97)

Since Cargo 1.91 the tree is split in two (defaulting to the same path):

- **artifact-dir** (`target/`) — final artifacts; "part of the public API".
  `target/<profile>/` holds uplifted binaries, `examples/`, `doc/`.
- **build-dir** — everything else; "an internal implementation detail":
  - `.rustc_info.json` (underscore — the `layout.rs` doc comment saying
    `.rustc-info.json` is a typo; the code writes underscore)
  - `<profile>/.fingerprint/<pkg>-<HASH>/` — freshness data per unit:
    `lib-<name>` (16-hex fingerprint), `lib-<name>.json`, `dep-lib-<name>`
    (binary dep-info), `invoked.timestamp`, `output-lib-<name>` (cached
    diagnostics — deleting it silently loses warnings)
  - `<profile>/deps/` — `.rlib`/`.rmeta`/`.d` per unit, disambiguated by
    `-C extra-filename=-<HASH>`
  - `<profile>/build/<pkg>-<HASH>/` — build-script binaries and their `out/`
    dirs ("the package shows up twice with two different metadata hashes" —
    compile-script and run-script are distinct units)
  - `<profile>/incremental/` — rustc-owned session dirs (see below)

Per-profile duplication multiplies all of it: `debug/`, `release/`, custom
profiles, plus a full extra tree per `--target` triple.

## The hashes

The `.fingerprint/<pkg>-<H>/` directory hash and the `-<H>` suffix in
`deps/` are **the same value** (`Metadata::unit_id` — verified in
`compilation_files.rs`: `c_extra_filename()` and `pkg_dir()` both return
`self.unit_id`). This is what makes hash-keyed sweeping possible at all.

Two caveats:
- **dylibs and uplifted top-level binaries carry no hash** — units without
  metadata share fingerprint dirs; suffix-matching misses them.
- The hash covers package id, post-unification features, profile, rustc
  version, target triple, RUSTFLAGS and all transitive dep hashes — so *any*
  of those changing orphans the entire previous artifact set.

## `incremental/` — partially self-cleaning, with a permanent leak

Cargo passes one directory per profile (`-C incremental=target/debug/incremental`);
rustc owns the contents. rustc **does** GC: it deletes abandoned `-working`
session dirs and keeps only the newest finalized session per crate,
coordinated by per-session-dir file locks (broken on NFS by design).

The leak: rustc only prunes crates it compiles **in that session**. The
`<crate>-<hash>/` dir of a dependency you removed months ago is never
revisited — it persists forever. (Inferred from rustc `persist/fs.rs`;
corroborated by `cargo clean -p` having to delete these itself.)

## Timestamps are deliberately unreliable

- **Fingerprint mtimes are backdated**: "When a build is complete, the mtime
  of the dep-info file in the fingerprint directory is modified to rewind it
  to the time when the build started" (fingerprint module doc). mtime ≠
  last-use.
- **atime is dead**: off by default on Windows
  (`NtfsDisableLastAccessUpdate`), `relatime` on Linux, and Cargo does not
  touch artifacts it reuses. See [06-cleanup-policies.md](06-cleanup-policies.md).

## Marker files

- `CACHEDIR.TAG` (signature `8a477f597d28d172789f06886806bc55`) is written to
  `target/debug|release|doc/` — **not** the `target/` root — and **not
  created if the directory already existed** (#12441: rust-analyzer often
  gets there first). Absence does not mean "not a target dir".
- Cargo 1.96 added its own validation before `cargo clean --target-dir`:
  refuse unless a valid `CACHEDIR.TAG` regular file (not symlink) is present.
  Targone should adopt the same guard before destructive operations.

## Scale of the waste (upstream data)

- rust-lang/rust#66348: typical target dirs 2–4 GB vs ~100 MB deployed
  binary (40×); "the majority of that is debug info"; a 4 GB target dir
  7-zips to ~700 MB — **~83% redundancy**.
- cargo#14125 (build-dir split): for cargo itself, `target/debug` drops from
  **4.2 GB to 415 MB** when intermediates move to a separate build-dir —
  i.e. ~90% of bytes are intermediates, not artifacts.
- Multiply by N projects × M profiles × toolchain updates every 6 weeks, and
  TB-scale accumulation on an active dev machine is the expected outcome,
  not an anomaly.

## The moving target: build-dir layout v2

`-Zbuild-dir-new-layout` (#15010) restructures build-dir into
`build/$pkgname/$META/{out,fingerprint,run}` — self-contained per unit,
which is precisely what makes GC tractable upstream. Timeline: default on
nightly 2026-07-24, re-stabilized mid-Aug 2026, **milestone 1.99
(~Oct 2026)**. Consequences for Targone:

1. Any tool hardcoding `.fingerprint/` + `deps/` + `build/` (all of them do)
   breaks on v2.
2. Targone must ship **dual-layout support from day one** — the old layout
   becomes legacy within weeks of our launch, but remains on disk in every
   existing `target/`.
