# 05 — Locking and safety: how to delete without corrupting a build

No existing tool takes any lock (verified for cargo-sweep, kondo,
cargo-clean-all, cargo-wipe). Even `cargo clean` takes none — its source
says "we're just going to blow it all away anyway", which is not a pattern
to copy. Being the first tool with real lock discipline is a core Targone
feature.

## Cargo's lock protocol (since 1.96 — three files, per profile dir)

Verified from `layout.rs` (`Layout::new`), Cargo 1.97:

| File | Location | Mode during a build |
|---|---|---|
| `.cargo-build-lock` | `<build-dir>/[<triple>/]<profile>/` | **exclusive** (shared under `-Zfine-grain-locking`) — the real build lock |
| `.cargo-lock` | `<artifact-dir>/[<triple>/]<profile>/` | **shared, always** — back-compat interlock with pre-1.96 cargos (which took it exclusively) |
| `.cargo-artifact-lock` | `<artifact-dir>/[<triple>/]<profile>/` | exclusive, only when artifacts are produced (`cargo check` skips it) |

Held for the entire build (the `FileLock` lives in `Layout`, owned by the
`BuildRunner`). **Locking `.cargo-lock` exclusively — what an older tool
would do — no longer blocks a modern build.**

## How Targone takes the locks — zero dependencies

Cargo now uses `std::fs::File::{lock, try_lock, lock_shared}` (stable since
Rust 1.89) over `flock(2)`/`LockFileEx` — an external tool is byte-compatible
with plain std:

```rust
// per profile dir, before any deletion in it:
let build_lock = File::options().read(true).write(true).create(true)
    .open(profile_build_dir.join(".cargo-build-lock"))?;
build_lock.try_lock()?;            // exclusive — else skip this dir this run
let compat = File::options().read(true).write(true).create(true)
    .open(profile_artifact_dir.join(".cargo-lock"))?;
compat.lock_shared()?;             // interlock with pre-1.96 cargos
```

A concurrent `cargo build` then blocks with Cargo's own message ("Blocking
waiting for file lock on build directory"). Policy: **`try_lock` and skip**,
never block a user's build waiting for GC. Model the overall protocol on
Cargo's `CacheLockMode::MutateExclusive` — its own answer for "a GC mutating
a cache others read".

Do **not** use the `cargo` crate or `cargo-util` as a library (`cargo-util`
exposes no FileLock and warns it may break without notice).

### Lock caveats

- **NFS: Cargo skips locking entirely** (`is_on_nfs_mount` → `None`). On
  network filesystems there is no safety protocol to join — detect and
  refuse to GC, or require an explicit flag.
- Filesystems returning `ENOTSUP`/`Unsupported`: Cargo proceeds unlocked;
  Targone should treat lock failure as "skip", not "proceed".

## Safe deletion order

Cargo's staleness rule (fingerprint module): "If any output file is missing,
then the unit is stale." So a missing artifact degrades to a rebuild —
*provided no fingerprint claims freshness over half-deleted outputs*.

**Rule: delete the unit's `.fingerprint/<pkg>-<H>/` directory FIRST, its
artifacts in `deps/`/`build/` SECOND.** Crash between the two steps leaves
orphaned artifacts (wasted bytes, harmless, next run re-collects) instead of
a stale fingerprint over missing files (silent broken-build risk).

Additional guards:

- Validate `CACHEDIR.TAG` (signature `8a477f597d28d172789f06886806bc55`,
  regular file, not symlink) in the profile dir before destructive work —
  same guard Cargo 1.96 added to `cargo clean` — but remember absence is
  possible on dirs created by rust-analyzer first; fall back to structural
  checks (`.fingerprint/` present, `.rustc_info.json` in build-dir root).
- Never follow symlinks out of the target tree.
- Dry-run mode as a first-class citizen; journal every deletion batch.

## Windows-specific hazards (primary dev platform)

Deletion on Windows is materially harder than Unix (where `unlink` on an
open file is safe — the inode outlives the fd):

- `ERROR_SHARING_VIOLATION (32)`: any handle opened without
  `FILE_SHARE_DELETE` blocks deletion; deletion is delete-*pending* (the
  name lingers until the last handle closes).
- **rustc memory-maps `.rmeta`** (rust#55556); a mapped file cannot be
  deleted at all (`ERROR_USER_MAPPED_FILE`).
- **A running executable pins its image** — and because the uplifted binary
  is a *hardlink* to `deps/…`, a running `cargo run` binary pins both names
  (cargo#12485, os error 5).
- Antivirus (Bitdefender notoriously) causes transient os error 5 on
  build/clean paths (cargo#11544, #14788).

Mitigations, in order: hold the build lock (removes the biggest handle
source); open-for-delete with `FILE_SHARE_DELETE` +
`FILE_DISPOSITION_POSIX_SEMANTICS` (Win10 1709+); rename-to-trash-subdir
then delete (renames succeed where deletes fail); **retry with backoff and
tolerate residue** — a leftover file is a bug report, not a failed run.
Consider the `remove_dir_all` crate (what cargo-clean-all uses) for whole-dir
tiers.

## Concurrency with rustc's incremental GC

`incremental/` session dirs have their own flock protocol (shared for
readers, exclusive for rustc's collector, "only directories older than a
few seconds are considered"). When pruning stale incremental dirs, take the
session dir's own lock exclusively first — same skip-on-failure policy.

## Layout v2

All paths above are the legacy layout. Under `-Zbuild-dir-new-layout`
(default ~1.99), fingerprint+outputs collapse into
`build/$pkgname/$META/{out,fingerprint,run}` — deletion becomes "remove one
unit directory", which is *safer* (atomic per unit, no cross-dir ordering).
The lock files stay in the profile dirs. Ship both code paths; select by
probing which structure exists.
