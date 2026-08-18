# 02 — Prior art: every existing tool, and why none solves this

## Summary matrix

| Tool | Deletes | Decision basis | Takes Cargo lock? | Windows | Usable as library? | Maintained? |
|---|---|---|---|---|---|---|
| cargo-sweep 0.8 | hash-keyed entries in `deps/`, `build/`, `.fingerprint/`, profile root | fingerprint `rustc` hash **or atime** | **No** | atime modes semantically broken | No (`has_lib=false`) | **Self-declared unmaintained** |
| kondo 0.9 | whole `target/` | marker file + mtime | No | good | **Yes (`kondo-lib`)** | Yes |
| cargo-clean-all 0.6 | whole `target/` | project-level mtime/size filter | No | best deleter (`remove_dir_all` crate) | No | Low |
| cargo-wipe 0.4 | whole `target/` | dir named `target` containing `.rustc_info.json` | No | ok | No | Low |
| cargo-cache / cargo-trim | `$CARGO_HOME` only | n/a | n/a | ok | No | — |
| sccache 0.17 | nothing (adds a cache) | compiler-level memoization | n/a | ok | n/a | Yes |
| cargo-mark-sweep 0.2 | non-live hashes + `incremental/` | `--message-format=json` live-set | claims yes (unverified) | **one-shot unsupported** | No | new, unproven, 51 downloads |

## The key negative finding

**No published crate prunes `target/` as a project dependency.** Verified
four independent ways: GitHub code search returns zero `build.rs` files
invoking any cleanup tool; reverse-dependency counts on crates.io are 0 for
every target-cleanup crate; `cargo-sweep` has no library target on any
published version; and no crate self-describes as build-time pruning across
~15 query variations. The one real library, `kondo-lib`, is
`remove_dir_all("target")` — useless mid-build.

## cargo-sweep — the closest attempt, and its three lessons

The only tool doing *selective intra-target* pruning. Hash-keyed sweep: parse
the 16-hex-digit suffix from filenames under `.fingerprint/`, build a
keep-set, delete everything else across `deps/`, `build/`, profile root and
`.fingerprint/`. README declares the project unmaintained.

**Lesson 1 — toolchain sweep works.** `--installed`/`--toolchains` hash
`rustc +<tc> -vV` output with **both** `rustc-stable-hash::StableSipHasher128`
(Cargo ≥1.85) and the legacy `SipHasher::new_with_keys(0,0)` (pre-1.85), plus
an unconditional `insert(0)` (build-script fingerprints claim rustc-hash 0),
and match against the `rustc: u64` field in fingerprint JSON. Fail-open when
JSON is unreadable. These modes are sound on Windows and are the only part
of cargo-sweep worth inheriting.

**Lesson 2 — atime-based modes are broken, and stayed broken for 8 years.**
`--time`/`--stamp`/`--file`/`--maxsize` all key on `metadata().accessed()`.
Issue #11 (open since 2018): Cargo doesn't update atime on cache reuse, so
after a stamp, a build, and a sweep, "everything gets removed :(" (the
author's own words). #3 (open since 2018): Windows disables atime by default.
Any time-based design inherits this.

**Lesson 3 — blind spots and no locking.** It explicitly skips `examples/`
and `incremental/` (often the largest dir). `--maxsize` measures the budget
over the whole target dir including dirs it cannot delete — if
`incremental/` alone exceeds the budget it deletes every tracked artifact
and still fails. It takes no lock of any kind (verified: no flock dep, no
lock file opened) and swallows deletion errors — it can race a live build.

## Whole-directory deleters (kondo, cargo-clean-all, cargo-wipe)

All three are `rm -rf target` with different discovery UX. kondo's README is
honest: "essentially `rm -rf` with a prompt". cargo-clean-all's
`--keep-days`/`--keep-size` are *project-level exclusion* filters, not
intra-target retention (easy to misread), but it uses mtime (not atime) and
the hardened `remove_dir_all` crate — the best deleter of the group.
cargo-wipe detects targets by `.rustc_info.json` presence — which breaks
when `build.build-dir` moves that file elsewhere. None take locks. Useful
role: the **idle-project wipe** tier, not the daily driver.

## `$CARGO_HOME` tools — out of scope, complementary

cargo-cache and cargo-trim manage registry/git caches only, and since Cargo
1.88 the built-in auto-GC covers that. They never touch `target/`.

## sccache — confirmed: does NOT reduce `target/`

It's a `RUSTC_WRAPPER`; rustc's `--out-dir` still points into `target/`. On
a hit the cached bytes are **byte-copied** into `target/` (no hardlink/
reflink); on a miss a second copy lands in `SCCACHE_DIR` (default 10 GB).
Net: N unchanged target dirs + an extra cache. Also: bin/dylib/cdylib/
proc-macro crates are never cached, incremental must be off, `cargo check`
is largely uncached. Solves compile *time*, worsens disk.

## Architecturally interesting corpses and newcomers

- **oxalica/cargo-gc-target** (archived 2022, yanked): attempted a true
  tracing GC; documented why it's hard — shared target dirs make artifacts
  reachable from *other* workspaces untraceable, and "Cargo `target`
  hierarchy and metadata calculation may change between versions" (it pinned
  cargo 1.51). Deprecated itself in favor of cargo-sweep.
- **cargo-overstay** (PATH shim wrapping `cargo`): runs cleanup in a
  background worker **after Cargo exits** — architecturally the right
  placement (outside the build).
- **cargo-mark-sweep** (2026, 51 downloads): mark phase runs the build with
  `--message-format=json` and harvests `compiler-artifact` messages as the
  live-set — the stable-API version of the `--build-plan` idea cargo-sweep
  wanted in 2018. Sweeps `deps/`+`build/` non-live hashes, wipes
  `incremental/` wholesale, and **never touches `.fingerprint/`** (claims
  some unit types have fingerprint hashes appearing in no artifact filename
  — directly contradicting cargo-sweep, which does delete there). Claims to
  hold Cargo's lock (unverified). Windows one-shot unsupported. Unproven,
  but the *design* is the best in the field.
- A 2026 wave of low-adoption crates (cargo-target-gc, cargo-reclaim,
  dev-prune, …, 15–201 downloads, several with no repo): flagged, not
  load-bearing for any conclusion.

## What the field teaches Targone

1. The empty niche is real: selective + safe + cross-project + Windows-sound
   exists nowhere.
2. Inherit cargo-sweep's toolchain-hash keep-set and cargo-mark-sweep's
   JSON-message live-set; reject every atime/mtime policy.
3. Place the engine **outside the build** (overstay's shim / a scheduled
   task), never inside it.
4. Take Cargo's real locks — being the only tool that does is a feature.
