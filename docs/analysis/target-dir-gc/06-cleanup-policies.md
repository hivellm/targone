# 06 — Cleanup policies: what to delete and how to decide

Catalog of every candidate policy, with a verdict. The engine composes the
accepted ones into tiers (cheap/safe first, aggressive last) under a global
size budget.

## Rejected signals

### atime — dead
Off by default on Windows (`NtfsDisableLastAccessUpdate`), `relatime` on
Linux, and Cargo never touches files it reuses from cache. Root cause of
cargo-sweep #11, open since 2018, with the author reproducing total cache
loss on three OSes. Never build on atime.

### mtime — unreliable inside `target/`
Cargo *deliberately backdates* fingerprint mtimes to build start ("to handle
the case where the user modifies a source file while a build is running"),
and reused artifacts keep their original mtime forever. mtime is usable for
exactly one thing: coarse "has anything in this tree changed in N months"
evidence for the idle-project tier — never per-artifact retention.
(`-Zmtime-on-use` would fix this, but is nightly-only and ignored on stable
— exploit when present, never require.)

## Accepted policies

### P1. Incremental-cache pruning — cheap, biggest easy win
`target/<profile>/incremental/` is often the single largest dir and is
purely a compile-speed cache — deleting it can never break correctness,
only slow the next build. rustc self-GCs sessions per crate but **never
removes dirs of crates no longer built** (see 01). Policy:
- delete session dirs of crates absent from the current lockfile/workspace;
- delete abandoned `-working` dirs older than a day;
- under size pressure or for idle projects, wipe `incremental/` wholesale
  (cargo-mark-sweep does this; so should we).
Respect rustc's per-session-dir flock (see 05).

### P2. Stale-toolchain sweep — proven sound (cargo-sweep's good half)
Keep-set = hashes of every installed toolchain's `rustc -vV` output, hashed
with **both** `rustc-stable-hash` (≥1.85) and legacy
`SipHasher::new_with_keys(0,0)` (<1.85), **plus the literal 0** (build-script
fingerprints report rustc-hash 0). Delete units whose fingerprint `rustc`
field matches no installed toolchain. Fail-open on unreadable fingerprints.
Windows-sound, no build required. Runs automatically after every rustup
update — the 6-week cadence that silently doubles every target dir.

### P3. Mark & sweep against the live-set — the precision tier
The only *supported* unit→file mapping Cargo offers:
`cargo build --message-format=json` emits `compiler-artifact` messages with
a `filenames` array (absolute paths) and `fresh` flag, plus
`build-script-executed` with `out_dir`. Mark = run the project's configured
build commands (default `cargo build` + `cargo build --release` when a
`release/` dir exists; user-extendable to `--all-targets`, `--all-features`)
and union the filename sets. Sweep = delete hash-suffixed entries in
`deps/`/`build/` not in the live-set, mapping hashes via the
`unit_id` ≡ extra-filename identity (see 01).

Precautions learned from prior art:
- **Do not delete from `.fingerprint/` in this tier** (cargo-mark-sweep's
  finding: some unit types' fingerprint hashes appear in no artifact
  filename; deleting them invalidates live crates → full rebuild). P2 (which
  matches fingerprints directly) is where fingerprint dirs get removed —
  and there, fingerprint-first ordering per 05.
- Unhashed outputs (dylibs, uplifted binaries) are never swept by suffix.
- A "shakedown" verify mode (re-run build, expect 100% `fresh`) for CI and
  for our own test suite.
- Cost: requires running a build → this tier is opt-in-per-run or scheduled
  (e.g. weekly), not per-GC-tick.

### P4. Size budget — the user-facing contract
The headline knob: `targone.toml → budget = "20GB"` per machine (and
optional per-project). When exceeded, escalate tiers over the projects
ranked by (idleness × size): P1 everywhere → P2 everywhere → P3 on the
fattest actives → P5 on the idlest. **Budget must be measured over bytes
the engine can actually delete** — cargo-sweep's `--maxsize` counts dirs it
refuses to touch and can therefore delete everything and still miss its
target; never repeat that.

### P5. Idle-project handling — where the TBs actually are
The registry (build.rs pings + `cargo targone scan` for never-registered
projects) gives last-activity per workspace. Tiered by idleness, all
thresholds configurable:
- idle > 30d: P1 + P2 + drop `debug/` artifacts of the workspace's own
  crates (deps keep-warm optional);
- idle > 90d: full `cargo clean`-equivalent wipe of the target dir
  (fail-safe: it's only ever compile time);
- workspace manifest gone (project deleted/moved): target/build dirs are
  **orphans** — reclaim fully. This is Cargo's own accepted #13136 model
  (registry of manifest-path ↔ target-dir ↔ timestamp), which we implement
  years ahead of upstream, deliberately compatibly.

### P6. Doc/examples/package residue — trivial adjuncts
`target/doc` (regenerable, often GBs), `target/package` (tarball verify
residue), `examples/`: age/idleness-gated wholesale deletion. Prior tools
simply skip these.

## Explicitly out of scope

- **Content-addressed cross-project artifact cache** — upstream's funded
  2026 goal; we'd be obsolete on arrival (see 03). Our horizontal play is
  central `build.build-dir` + P4/P5 over the central root.
- **Compression/dedup of live artifacts** (the 83%-redundancy temptation):
  filesystem-dependent (FIDEDUPERANGE/reflink absent on NTFS; Windows has
  its own dedup only on Server), high corruption stakes. Revisit post-1.0
  at most.
- **sccache integration** — orthogonal (compile time, not disk; actually
  adds disk).

## Config surface (draft)

```toml
# targone.toml (per project) / ~/.targone/config.toml (machine)
budget = "20GB"            # machine-wide cap for all managed dirs
keep_warm = true           # prefer P1–P3 over wipes for active projects
idle_after = "30d"
wipe_after = "90d"
profiles = ["debug", "release"]
mark_commands = ["build", "build --release"]   # P3 live-set commands
central_build_dir = false  # opt-in migration (see 04-F)
```
