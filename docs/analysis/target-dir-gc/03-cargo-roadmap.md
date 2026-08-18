# 03 — Cargo's own roadmap: what ships, what's accepted, what's blocked

Baseline: stable = Cargo/Rust **1.97.0** (2026-07-09); 1.98 ~2026-08-20;
1.99 ~2026-10-01. There is **no GC RFC** — the work runs through a HackMD
design doc + tracking issue #12633. (RFC 3537 is the MSRV resolver, a
common mix-up.)

## Shipped (stable)

| Version | Date | What |
|---|---|---|
| 1.78 | 2024-05 | Last-use tracking for `$CARGO_HOME` |
| **1.88** | 2025-06 | **Automatic global-cache GC**: network files unused 3 months / local files 1 month are removed; `cache.auto-clean-frequency` default `"1 day"` |
| 1.91 | 2025-10 | **`build.build-dir` stabilized** — artifact-dir/build-dir split |
| 1.93 | 2026-01 | `target/package` gets CACHEDIR.TAG |
| 1.96 | 2026-05 | `cargo clean` validates CACHEDIR.TAG before deleting; **build-lock split** (`.cargo-build-lock` / `.cargo-lock` / `.cargo-artifact-lock`) |

**Scope of the 1.88 auto-GC is `$CARGO_HOME` only** — registry sources,
crate tarballs, git checkouts. It does not touch `target/`. The part of the
problem Cargo has solved is not the part that fills the SSD.

## Accepted but unbuilt: `target/` GC

- **#13136 "Garbage collect whole `target/`"** — open, `S-accepted`,
  assigned. Model: a GC database of *(root manifest path, target dir,
  timestamp)* — note neither field can be a primary key (many workspaces ↔
  one target dir). Modes: delete unused-for-X, delete-all post-toolchain-
  upgrade, delete leaked dirs of deleted workspaces. **Whole-dir GC only** —
  no intra-target pruning.
- **#5026** (2018, open): "target fills with outdated artifacts as
  toolchains are updated" — the intra-target problem, blocked on layout.
- **#6229** (2018): "have Cargo clean up after itself" — closed as duplicate.
- `cargo clean gc` (`-Zgc`) exists on nightly for `$CARGO_HOME` but is
  explicitly **not proposed for stabilization** (#13060, `S-needs-design`);
  the issue's own open question: "How does this evolve with cleaning target
  directories?"

Reading: upstream **will eventually ship whole-target-dir GC keyed on a
project registry** — the same shape as Targone's idle-project tier. That
validates the design and bounds Targone's long-term scope: our lasting
differentiation is the *intra-target* tiers and cross-project management,
not rediscovering dead `target/` dirs.

## The enabler landing now: build-dir layout v2

`-Zbuild-dir-new-layout` (#15010) groups build-dir content **per unit**
(`build/$pkgname/$META/{out,fingerprint,run}`) — built explicitly to enable
GC, fine-grained locking, and cross-project caching. Default on nightly
since 2026-07-24; milestone **1.99 (~Oct 2026)**. Targone must handle both
layouts (see [01](01-problem-mechanics.md)).

## The 2026 endgame: cross-workspace build cache

**Rust Project Goal 2026 "Cargo cross workspace cache"** — accepted, funded
(~$30k AWS), owner ranger-ross, champion Ed Page. Content-addressed shared
cache giving "the benefits of a shared `CARGO_TARGET_DIR` out of the box".
2026 plan: initial nightly support for **basic crates only** (no build
scripts, no proc-macros); **stabilization explicitly out of scope for the
goal period**; no `-Z` flag exists yet. Prerequisites still open:
nondeterministic codegen at `codegen-units=16` (rust#128675) and Cargo's own
metadata files not being byte-stable (#16693).

Reading: the *horizontal* problem (N projects × same deps compiled N times)
is being solved upstream on a **2027+ stable horizon**. Targone should not
build a content-addressed artifact cache — it would be obsolete on arrival.
Bridge the gap with configuration (`build.build-dir` centralization) and
dedup instead.

## Unstable flags relevant to Targone

- **`-Zmtime-on-use`** (#7150) — "an experiment to have Cargo update the
  mtime of used files to make it easier for tools like cargo-sweep to detect
  which files are stale." The sanctioned freshness signal — but nightly-only
  and ignored on stable, so Targone can exploit it when present, never
  require it.
- **`-Zno-embed-metadata`** (#15495) — stop duplicating metadata into both
  `.rlib` and `.rmeta`. Measured (Kobzol, hyperqueue): **release −36%**,
  dev −9..18%. Nightly-only; a free win to surface to users, not depend on.
- `-Zfine-grain-locking`, `-Zchecksum-freshness`, `-Zbuild-analysis`
  (JSONL build logs with rebuild reasons in `$CARGO_HOME/log/` — a potential
  future activity signal).

## Hooks Cargo will NOT give us

- **Post-build hooks: rejected.** #545 open since 2014 (`E-hard`,
  `S-needs-design`); RFC 1777 rejected after FCP in 2017 ("possible with a
  `cargo-something` command — try that first"). 2025 direction (#14948) is
  explicitly to *reduce* build-script usage.
- **Target-dir path for build scripts: refused.** #9661 is `S-propose-close`;
  ehuss: build scripts' "only interaction should be through the `OUT_DIR`".

Both refusals shape the integration design in
[04-integration-mechanisms.md](04-integration-mechanisms.md).
