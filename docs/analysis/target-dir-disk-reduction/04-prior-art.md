# 04 — Prior art: what already exists and why it is not enough

Survey of `cargo-sweep`, `kondo`, `cargo-cache`, `cargo-clean-all`, `cargo-wipe`,
`cargo-trim`, `sccache`, the archived `cargo-gc-target`, and Cargo's own GC work.
Source-verified except where flagged.

---

## F-030 — No crate exists that does this as a dependency. The niche is genuinely empty

Four independent checks, all negative:

1. GitHub code search for a build script doing target cleanup returns **zero**
   hits: `cargo-sweep path:build.rs`, `cargo-clean-all path:build.rs`,
   `remove_dir_all + target + path:build.rs lang:rust`,
   `CARGO_TARGET_DIR + remove_dir_all + path:build.rs`.
2. crates.io reverse-dependency counts are **0** for every target-cleanup crate
   (`cargo-target-gc`, `cargo-reclaim`, `clean-dev-dirs`, `dev-prune`). Only
   `kondo-lib` has any (3), and it is a whole-directory deleter.
3. **`cargo-sweep` has no library target at all** (`has_lib = false`, every
   version) — it cannot be called from anywhere, let alone a build script.
4. No crate on crates.io describes itself as build-time or dependency-invoked
   pruning, across ~15 query variations.

The closest historical attempt, `oxalica/cargo-gc-target`, is **archived and
yanked**, its README redirecting to `cargo-sweep`. It attempted a true tracing
GC and its own documented limits name the hard parts precisely: *"doesn't work
well on shared target directory, since a simple tracing GC will erase all
artifacts untraceable from current workspace but may still be referenced in some
other workspaces"* and *"Cargo target hierarchy and metadata calculation may
change between versions."*

**Impact.** Targone is not reinventing an existing crate — the design space it
targets is unoccupied. But an empty niche in a nine-year-old ecosystem with
2,375-star tools nearby is evidence about difficulty, not opportunity, and the
rest of this file explains why: the mechanism the problem statement asks for
(cleanup from inside the build) is structurally unsafe (F-036), and the signal
the obvious implementation would use (`atime`) is structurally unreliable
(F-033).

**Confidence: high** for the negative result (four independent methods).

---

## F-031 — `cargo-sweep` is unmaintained and explicitly skips `incremental/` — 67% of the bytes

MIT, v0.8.0 (2025-10-11), 980 stars, 994k downloads, 30 open issues. **The
README states the project is unmaintained with no dedicated maintainer**
(issue #68); the last commit (2026-05-26) touched only the README and clippy
lints. Release cadence: 0.6.2 (2021) → 0.7.0 (2023) → 0.8.0 (2025).

Its sweep operates on hash identity, not directories: `hash_from_path_name()`
cuts the filename at the first `.`, `rsplit('-')`, and keeps the last segment
only if it is exactly 16 ASCII hex digits. It walks every `.fingerprint`
directory, builds a keep-set of hashes, and deletes non-matching entries from:

```
build/ , deps/ , the profile dir root , .fingerprint/ , native/ (legacy)
```

**Explicitly skipped: `examples/` and `incremental/`** — the source comments
that incremental is "not tracked by fingerprint".

**Impact.** This is the decisive gap. `incremental/` is **201.8 GB of the
300.8 GB** on this machine (F-002) and is **free to delete** (F-042). The most
popular tool in the space, already installed here (F-010), cannot touch the
largest and cheapest pool of bytes by design. That single fact justifies
Targone's existence more than any other in this analysis: a correct
`cargo-sweep` run on Cortex would reclaim at most 30 GB of 172 GB, while the
identity policy in F-044 reclaims 153 GB.

Encouragingly, cargo-sweep's hash-identity approach independently corroborates
the mechanism measured in F-005 — the same join key, arrived at separately.

**Confidence: high** (source-verified).

---

## F-032 — `cargo-sweep`'s toolchain attribution is the part worth borrowing

It does **not** read `.rustc_info.json` and does **not** use mtimes for this.
Instead:

1. `rustup toolchain list` enumerates installed toolchains (falling back to a
   bare `rustc` call when rustup is absent).
2. For each, it runs `rustc +<toolchain> -vV` and hashes the **raw stdout
   string**, inserting *two* hashes into the keep-set:
   - `rustc_stable_hash::StableSipHasher128` — the comment says *"has to match
     the way Cargo hashes a rustc version. As such it is copied from Cargos
     code."*
   - `std::hash::SipHasher::new_with_keys(0, 0)` — *"used prior to Rust 1.85.0"*
3. `toolchain_set.insert(0)` unconditionally: *"Some fingerprints made to track
   the output of build scripts claim to have been built with a rust that hashes
   to 0... this makes sure we don't clean the files."*
4. `Fingerprint::load()` reads any `*.json` in
   `target/<profile>/.fingerprint/<pkg>-<hash>/` and deserialises only
   `{ rustc: u64 }`; the first JSON that parses wins.
5. **Fail-open**: `f.unwrap_or(true)` — a unit whose JSON cannot be read is
   *kept*.

**Impact.** Two lessons, one positive and one cautionary:

- **Borrow the fail-open discipline and the dual-hash keep-set.** Fail-open is
  the correct default for a deletion tool and should be a stated invariant of
  Targone: any unit that cannot be positively classified as dead is kept.
- **Note the fragility it reveals.** Targone would be depending on an internal
  Cargo hash that has *already changed once* (Rust 1.85, PR #139 in
  cargo-sweep, 2025-04-13). Any design keyed on Cargo's internal hashing
  inherits a maintenance obligation on every Rust release. The identity policy
  in F-044 avoids this entirely — it compares hashes to *each other* for
  recency, never needing to know what a hash means.

**Confidence: high** (source-verified, with quoted comments).

---

## F-033 — `cargo-sweep`'s `atime` dependency has been broken for eight years, and Windows is the worst case

`--time N`, `--all`, `--file` and `--maxsize` all decide via `last_used_time()`,
the **minimum of `metadata().accessed()`** over a fingerprint directory's
entries. That is `atime`.

- **Issue #11, open since 2018**: "Cargo build does not update access times for
  existing files". The author reproduced it on Windows *and* macOS
  (`cargo clean; cargo build; cargo sweep -s; cargo build; cargo sweep -f` →
  *"Now everything get removed :("*), confirmed on Linux by a third party. The
  author's own verdict: *"with unmodified access times there is no way to
  distinguish what is being used or not."* Most recent comment **2026-01-06**:
  *"this issue makes --stamp/--file useless for keeping CI cache efficient and
  size controlled at the same time... only complete when used after a
  cargo clean."*
- **Issue #3, open since 2018**: "Let windows users know if atime won't work" —
  never implemented.

**Impact.** Independent, eight-year confirmation of F-047 from the tool with
the most field exposure. It also sharpens the conclusion: the failure is not
that `atime` is *missing*, it is that **Cargo does not touch `atime` when it
reuses a cached artifact**, so even a filesystem that updates `atime` faithfully
reports a live artifact as untouched. My F-008 measurement showed `atime` *does*
move for `.rlib`/`.rmeta` on this machine (they are re-read by rustc at link
time), which is why it works as a *sparing* signal — but F-033 explains exactly
why it must never be a *condemning* one.

Consequence for cargo-sweep: on default Windows configurations only
`--installed` and `--toolchains` are semantically sound. Every time-based mode
is unsafe there.

**Confidence: high** (issue threads + source).

---

## F-034 — `cargo-sweep --maxsize` measures a budget over directories it cannot delete

`remove_older_until_fits()` computes `starting_size` by walking the **entire**
target directory — including `incremental/`, `examples/` and `doc/`, none of
which it will delete (F-031). It then sorts tracked units by last-used and
removes oldest-first until `removed >= size_to_remove`.

**Impact.** If `incremental/` alone exceeds the requested budget — which is the
normal case here: 127 GB of Cortex's 172 GB — the tool will delete **every
tracked artifact** and still not reach its target. The user asked for a size cap
and received a `cargo clean`. This is the single most instructive bug in the
survey for Targone's design: **a size budget must only be computed over the set
the policy is actually able to reclaim.** F-048's formulation (budget as a
scheduler-level ordering function over *reclaimable* bytes, never as a deletion
criterion) is a direct response.

**Confidence: medium-high** — derived from source reading; not empirically
reproduced.

---

## F-035 — No established tool takes Cargo's lock; `cargo-sweep` can and does corrupt concurrent builds

Verified negative for `cargo-sweep`: its dependency list (clap, crossterm,
walkdir, rustc-stable-hash, anyhow, log, fern, cargo_metadata 0.9, serde,
human-size) contains **no** locking crate — no `fs2`, `fd-lock`, `flock`, or
`cargo-util`. No source file opens `.cargo-lock`, `.cargo-build-lock` or
`.cargo-artifact-lock`. It calls `fs::remove_file` / `remove_dir_all` directly
and only `warn!`s on error, so a partially-swept target directory is left
**silently**.

Issue #8 records the resulting failure shape on Windows:
`LINK : fatal error LNK1201` writing to a `.pdb`.

`kondo`, `cargo-clean-all`, `cargo-wipe`, `projclean`, `dua-cli` and `ncdu`:
none take any lock either.

**Impact.** Direct external confirmation of F-051 (locks protect nothing unless
you take them) and of the LNK1201 class of damage predicted by F-053. It also
establishes lock discipline as Targone's clearest correctness differentiator —
no existing tool has it, and the one tool claiming it (`cargo-mark-sweep`) makes
the claim only in a README.

**Confidence: high** (dependency manifest + source + issue thread).

---

## F-036 — Cargo holds the build lock exclusively, so a build script *cannot* acquire it. Build-time cleanup is structurally unsafe

From Cargo's `src/compiler/layout.rs` (`Layout::new`):

| lock | location | mode |
|---|---|---|
| `.cargo-build-lock` | `<build-dir>/[<target>]/<profile>/` | `open_rw_exclusive_create` **by default** (shared only under `-Z fine-grain-locking`) |
| `.cargo-lock` | `<artifact-dir>/[<target>]/<profile>/` | `open_ro_shared_create` — retained only for compatibility with tools predating `.cargo-build-lock` |
| `.cargo-artifact-lock` | `<artifact-dir>/[<target>]/<profile>/` | `open_rw_exclusive_create`, when `must_take_artifact_dir_lock` |

And critically: **locking is skipped entirely on NFS** — `if is_on_nfs_mount(..) { None }`.

**Impact.** This is the formal proof of what F-020 and F-022 showed
experimentally. Cargo holds `.cargo-build-lock` exclusively for the entire
build; a build script runs *inside* the holder, so it can never acquire the lock
that would make deletion safe. Any pruning performed from a build script races
the compiler that spawned it, by construction — no amount of care in the build
script can fix this.

Together with F-019 (a dependency's build script runs about once, ever) and
F-020 (forcing it to run more often recompiles the world), this closes the
question the problem statement opened: **a dependency crate must not clean
during the build.** It can only register.

The NFS carve-out is a second-order warning: on network storage there is no lock
at all, so Targone must detect NFS/SMB target directories and refuse to sweep
them rather than relying on a lock that Cargo silently declined to take.

**Confidence: high** (Cargo source, corroborated by my F-050 measurements).

---

## F-037 — The popular tools are whole-directory deleters: `cargo clean` with better ergonomics

| tool | stars | what it deletes | selectivity inside `target/` |
|---|---:|---|---|
| **kondo** v0.9 (2026-01-23), actively maintained | 2,375 | `fs::remove_dir_all` on `["target", ".xwin-cache"]` | **none** — README: *"essentially `rm -rf` with a prompt"* |
| **cargo-clean-all** v0.6.4 (2025-04-19) | 264 | the whole `target/` dir | **none** — `--keep-days`/`--keep-size` are *project-level exclusion* filters, not intra-target retention |
| **cargo-wipe** v0.4.0 (2025-11-16) | 181 | the whole `target/` dir | none |
| **projclean**, **clean-dev-dirs**, **tin-summer** | — | whole dirs | none |

`cargo-clean-all` deserves credit on two points: it decides with **mtime**
(`max(md.modified())`) rather than atime, sidestepping F-033 entirely; and it is
the best-hardened deleter of the group, using the `remove_dir_all` crate with
`features = ["parallel"]`, which exists specifically to work around Windows
deletion semantics (F-053). `kondo` ships a real library (`kondo-lib`) with a
usable API — but that API is `remove_dir_all("target")`.

**Impact.** These tools solve *discovery* ("find my target dirs and show me the
sizes") well and *retention* not at all. Every one of them leaves the user with
the cold-rebuild problem the problem statement names in its second paragraph.
Their popularity relative to cargo-sweep suggests users prefer a simple
guaranteed-correct wipe to a subtle tool they cannot trust — which is a signal
about how Targone must present itself: the selectivity is only valuable if the
user believes it is safe.

Two implementation details worth adopting: the `remove_dir_all` crate for
Windows-hardened deletion, and mtime over atime as the primary time signal.

**Confidence: high** (source-verified).

---

## F-038 — `cargo-cache` and `cargo-trim` are `$CARGO_HOME`-only and complementary, confirming F-009's scope

`cargo-cache` (MIT/Apache-2.0) operates on `$CARGO_HOME` only. Its `local`/`l`
subcommand **reports** on a project's `target/` but never cleans it. `cargo-trim`
(v0.16.0, 2026-07-18) likewise covers `$CARGO_HOME/registry` and `/git` only.

**Impact.** Confirms the measured scope split in F-009: these tools address the
3.5 GB `$CARGO_HOME`, not the 300.5 GB in target directories. They are
complements, not competitors, and Targone should say so explicitly rather than
positioning against them.

**Confidence: high.**

---

## F-039 — Cargo upstream has no `target/` GC, and the request was closed without implementation

- The `[gc]` config, `-Zgc`, `cache.auto-clean-frequency` and `cargo clean gc`
  work (rust-lang/cargo #13058-#13064) targets **`$CARGO_HOME` only**.
- **rust-lang/cargo#6229**, "Have an option to make Cargo attempt to clean up
  after itself" (2018, 30 reactions), was **closed without implementation**.

**Impact.** Confirms F-009's conclusion with upstream evidence: waiting for
Cargo is not a strategy. It also means Targone is not building something that
will be obsoleted by the next Cargo release — the upstream effort is
deliberately scoped elsewhere, and the request for target-dir GC has been
formally declined once.

**Confidence: high** for the `$CARGO_HOME` scope and the #6229 closure;
**medium** on precise current status of each linked issue (issue numbers
reported by survey, not each individually re-read here).

---

## F-040 — Two designs put the work *outside* the build; both are recent and unproven, and both point the same way

- **`cargo-overstay`** v0.3.0 — a **PATH shim**. You symlink
  `~/.cargo-overstay/bin/cargo` ahead of the real cargo; it forwards the
  invocation, and a background worker cleans **after Cargo exits**.
- **`cargo-mark-sweep`** v0.2.0 (51 downloads, 1 star) — a **mark phase** that
  runs builds with `--message-format=json` and harvests `compiler-artifact`
  messages to name the live set (*"Stable public interface; no cargo internals
  parsed"*), then a sweep phase that deletes non-live hashes from `deps/` and
  `build/` and **wipes `incremental/` wholesale**. It claims its daemon holds
  Cargo's `.cargo-lock` while working — the only tool claiming lock discipline,
  **README-only, source not verified**. Its platform table marks the one-shot
  command unsupported on **Windows**; the daemon is macOS/launchd only.

Note a direct contradiction worth resolving before implementing: cargo-mark-sweep
claims some unit types have fingerprint hashes that appear in no artifact
filename, so deleting from `.fingerprint/` invalidates live crates — while
cargo-sweep **does** delete from `.fingerprint/`. My own measurement (F-005)
found 3,569 fingerprint dirs against 3,371 distinct deps hashes, a 198-entry
excess consistent with cargo-mark-sweep's claim (build-script `run-build-script-*`
units produce fingerprints with no artifact of their own).

**Impact.** Three independent designs — cargo-overstay's shim, cargo-mark-sweep's
daemon, and this analysis's scheduler (F-028) — converge on the same conclusion:
**the work belongs outside the build.** That convergence is the strongest
available validation of the architecture in `07`.

Two concrete borrowings:
1. The **`--message-format=json` `compiler-artifact` stream** is a *stable,
   documented* way to name the live set, avoiding the internal-hash fragility of
   F-032. It requires running a build, so it cannot be used from inside one — but
   a scheduler-driven tool can record it opportunistically.
2. The **PATH-shim trigger** is a genuine fifth mechanism I had not enumerated in
   F-028: it fires exactly once per cargo invocation, after the build, with no
   build cost and no scheduler registration. It is more invasive than a scheduler
   entry (it intercepts every cargo call, and breaks if it misbehaves) but it
   solves the trigger problem without any per-project change at all.

**Confidence: medium** — these are low-adoption tools whose functional claims are
README-level; the *architectural* lesson is high confidence, the implementations
are not endorsed.

---

## F-041 — The `target/` layout every tool hardcodes is actively being restructured

- Cargo now splits **artifact-dir vs build-dir** (`build.build-dir`).
- **`-Zbuild-dir-new-layout`** (rust-lang/cargo#15010) restructures `build/` into
  `$pkgname/$META/{out,fingerprint,run}` — flattening the
  `.fingerprint/` + `deps/` + `build/` shape that **every** tool in this survey
  assumes.
- A related trap: Cargo's `layout.rs` doc comment says `.rustc-info.json`
  (hyphen); the real filename is **`.rustc_info.json`** (underscore), written at
  `ws.build_dir().join(".rustc_info.json")`. `cargo-wipe` keys its project
  detection on that exact filename — and with `build.build-dir` set elsewhere,
  the file no longer lives under `target/`, so `cargo-wipe` would fail to detect
  the directory at all (structurally implied; not empirically verified).

**Impact.** Layout knowledge is a depreciating asset and must be isolated behind
one module with explicit version detection and a fail-closed default: if the
layout is not recognised, sweep nothing and say so. It also raises the value of
the two layout-independent signals available: `CACHEDIR.TAG` for discovery
(F-055) and the `compiler-artifact` JSON stream for liveness (F-040). And it
argues against `.rustc_info.json` as a required discovery marker — F-055's
conclusion, reached independently, now has a second reason behind it.

**Confidence: high** for the layout changes; **medium** for the cargo-wipe
consequence (flagged unverified by the survey).

---

## Unverified items carried forward

Flagged by the survey and **not** relied on for any recommendation:
whether `cargo-sweep` 0.8.0 handles a custom `build.build-dir`;
`cargo-mark-sweep`'s `.cargo-lock` claim (README only); `cargo-wipe` under a
split build-dir; `cargo-sweep --maxsize` over-deletion (source-derived, not
reproduced); and all functional claims of a mid-2026 cluster of low-adoption
crates (`cargo-target-gc`, `cargo-orphan-gc`, `cargo-reclaim`,
`cargo-target-guard`, `oxicleaner`, `deepclean`, `dev-prune`, `fleet-warden`) —
15-201 downloads, 0-1 stars, several reading as machine-generated.
