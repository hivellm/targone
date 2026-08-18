# 02 — Anatomy of the growth: where the bytes come from

`01-measurements.md` established *how much* and *where*. This file explains *why*
those directories grow the way they do, because the growth mechanism determines
which cleanup policies are correct and which are merely plausible.

---

## F-011 — Hash-suffix churn is the growth engine; nothing ever removes the old suffix

Every compiled unit lands under a hash-suffixed name:
`deps/libtokio-65ef5888ecb646c3.rlib`, `.fingerprint/tokio-65ef5888ecb646c3/`,
`incremental/tokio-0mmlnna6sw4au/`. The suffix is derived from everything that
affects codegen: dependency versions, enabled features, profile settings,
`RUSTFLAGS`, target triple, and the compiler version itself.

When any input changes, Cargo computes a *new* suffix and writes a *new* set of
files beside the old ones. It never removes the old ones — there is no
bookkeeping anywhere in `target/` that says "hash X superseded hash Y".

Measured consequence (Cortex, `debug/`):

- `.fingerprint/`: **3,569** directories, all live-looking, for a workspace with
  roughly 200 buildable units — a ~18x accumulation
- `incremental/`: **8,113** directories for **196** crate names — a ~41x accumulation
- `deps/`: 34.5 GB of 42.4 GB (81%) is artifacts whose base name also exists with
  a newer hash

Concretely: `libcortex_workers` exists in 46 hash variants totalling 0.99 GB;
`libcortex_api` in 54 variants; `libsyn` machine-wide in 40 variants across 15
projects.

**Impact.** The dominant term is not "one project builds a lot of code", it is
"one project builds the *same* code under 20-40 different identities and keeps
every one". This is what makes a *relative* policy ("keep the newest N per
identity") both safe and enormously effective — it exactly matches the shape of
the waste, whereas an absolute policy (age, size) does not.

**Confidence: high** (direct measurement, `01-measurements.md` F-003/F-005).

---

## F-012 — Integration test binaries are the multiplier, and each carries a full debug payload

Cortex's `deps/` holds **696 `.exe`** (12.9 GB) and **730 `.pdb`** (25.3 GB).
The `incremental/` directory names show the same population:
`ac_disabled_passthrough_it-*`, `graph_communities_it-*`, `raw_proxy_acl_it-*`,
`law_check_it-*`, `consolidator_health_endpoint_it-*` — one integration test file
per binary, per Cargo's `tests/*.rs` convention.

Each integration test file becomes an independently linked executable that
statically includes the workspace and all its dependencies. Sixteen hash variants
of `graph_communities_it` cost 0.56 GB; the same for `raw_proxy_acl_it`,
`law_check_it`, `decision_lookup_it`, `governance_global_index_it` — 0.54-0.56 GB
each.

**Impact.** Test-heavy workspaces pay a multiplicative penalty: *(number of
integration test files) × (workspace + dependency closure size) × (number of hash
variants retained)*. This is why Cortex is 172 GB and a similarly-sized library
crate is 2 GB. It also means the biggest single lever after `incremental/` is
"stale linked binaries", which — per F-004 — are never read back by anything.

**Confidence: high** (direct measurement).

---

## F-013 — The `dev` profile is essentially the whole problem; `release` is a rounding error

| project | debug | release |
|---|---:|---:|
| Cortex | 171.8 GB | 0.2 GB |
| Thunder | 55.5 GB | 1.2 GB |
| ar-v3-dashboard | 44.9 GB | 0.0 GB |

**Impact.** The problem statement lists "per-profile duplication — `debug/`,
`release/` and any custom profiles each keep a full copy" as a driver. On this
machine that is not what is happening: release builds are rare and their output
is small (Cortex's release profile has `strip = true`, `lto = "thin"`,
`codegen-units = 1`). Effort spent on cross-profile deduplication would be
misdirected. A policy may safely treat profiles independently and will find
almost all of its work inside `debug/`.

**Confidence: high** for this machine; **medium** as a generalisation (a CI-style
machine or a project shipping many release builds would differ).

---

## F-014 — Cross-project sharing can save at most 16.3 GB (5.4%) — the "horizontal" dimension is a distraction

Every `.rlib` in every project on the machine, aggregated:

```
total .rlib files across all projects : 4,212 files, 20.1 GB
distinct crate names                  : 586
crate names present in >1 project     : 327
bytes if exactly ONE copy of each name existed machine-wide : 3.8 GB
redundant bytes                       : 16.3 GB
```

And much of even that 16.3 GB is *intra*-project churn, not cross-project
duplication: `libthunder_bench` accounts for 3.89 GB of redundancy across 32
copies **inside a single project**. The genuinely cross-project entries are
comparatively small — `libreqwest` 0.87 GB over 7 projects, `libtokio` 0.69 GB
over 10, `librustls` 0.65 GB over 9, `libsyn` 0.41 GB over 15, `libwindows_sys`
0.38 GB over 11.

**Impact.** This overturns a premise of the problem statement. The `.rlib` pool —
the *only* thing a shared `CARGO_TARGET_DIR`, a shared artifact cache, or
`sccache` can deduplicate — is **20.1 GB of 300.8 GB (6.7%)**, and perfect,
zero-cost, magically-safe deduplication of it would recover **16.3 GB (5.4%)**.

Compare with pruning superseded artifacts in place: **257.7 GB (85.7%)**
(see `05-policies.md`).

The "ten projects compile the same `tokio` ten times" intuition is true but
costs 0.69 GB. The architecture should therefore treat cross-project sharing as
an **optional later optimisation**, not as a pillar — and certainly not at the
price of the correctness and lock-contention hazards catalogued in
`03-integration-mechanisms.md` and `06-safety-and-concurrency.md`.

**Confidence: high** for the measurement; **high** for the conclusion (the gap is
16x, far outside any measurement error).

---

## F-015 — rustc prunes inside an incremental crate directory; nothing prunes across them

Sampling incremental crate directories shows a consistent 1-2 `s-*` session
subdirectories each:

```
ac_disabled_passthrough_it-00r90v33mv5ac -> 2 sessions
ac_disabled_passthrough_it-011dbq6kz0mf0 -> 2 sessions
ac_disabled_passthrough_it-09cr169w9ykwt -> 2 sessions
```

rustc's incremental engine finalises a session and deletes superseded sessions
*within* the crate directory it was told to use. It has no visibility of, and no
mandate over, sibling directories belonging to other disambiguators — those are
Cargo's naming, not rustc's.

**Impact.** The bound that exists (sessions per crate dir) is enforced at the
wrong level. The unbounded axis — number of crate dirs per crate name — is owned
by nobody: rustc does not know the set, and Cargo does not garbage-collect. The
8,113-for-196 ratio is the direct consequence. A GC operating at the crate-name
level fills exactly the gap neither component covers, which is a good sign the
layer is the right one.

**Confidence: medium-high** (behaviour observed; mechanism corroborated in
`04-prior-art.md`).

---

## F-016 — Windows amplifies the problem by ~15% through mandatory per-artifact PDBs

45.1 GB of the 300.8 GB machine-wide total is `.pdb` files. In Cortex alone,
`.pdb` is 25.3 GB — larger than every `.rlib`, `.rmeta`, `.exe` and build-script
output in that project combined.

This persists despite `[profile.dev] debug = "line-tables-only"` being set. The
`x86_64-pc-windows-msvc` target emits debug information into a separate PDB per
linked artifact by design; there is no packed/unpacked `split-debuginfo` choice
on MSVC comparable to what ELF targets offer.

**Impact.** Two consequences for the design:

1. A meaningful slice of the win is Windows-specific and comes from a file class
   that is *provably* write-only (0 of 730 PDBs were ever re-read, F-004). PDBs
   are the safest bytes on the disk to delete and should be a first-class
   category in the policy engine, not an incidental one.
2. Cross-platform expectations must be set explicitly: the same policy on Linux
   will free proportionally less from this category, and the "delete stale
   binaries" category will behave differently because ELF binaries carry
   debuginfo inline (deleting the binary reclaims it; there is no separate file).

**Confidence: high** (direct measurement).

---

## F-017 — Abandoned and vestigial target directories exist and no age policy will ever visit them

Three of the 18 directories found are husks: `Lexum/crates/lexum-core/target` and
`Synap/crates/synap-server/target` contain **0 files**;
`Nexus/crates/nexus-core/target` contains 32. Meanwhile the whole-directory scan
turned up **48** directories literally named `target`, of which 30 were
false positives from vendored LLVM/GCC sources inside `E:\HiveLLM\Tml`
(`llvm/lib/Target`, `lldb/source/Target`, ...).

**Impact.** Two design requirements fall out:

1. **Name-matching alone would have pointed a deletion tool at LLVM source
   trees** — an unacceptable failure mode for a tool with delete authority. A
   marker-based discriminator is mandatory. But the obvious marker,
   `CACHEDIR.TAG`, is *not* sufficient on its own: it is absent from the 172 GB
   Cortex directory. F-055 derives the composite test that works.
2. **A dependency-crate-only design cannot reach abandoned projects.** A build
   script runs only when the project is built. A project you have stopped
   building is precisely the one whose GBs you most want back, and it will never
   run your code again. This is a structural argument that the build-time hook
   must be paired with something that has machine-wide reach — see
   `03-integration-mechanisms.md`.

**Confidence: high** (direct observation).

---

## F-018 — Existing per-project `.cargo/config.toml` files are mutually incompatible in ways that block naive sharing

Seven projects already carry a `.cargo/config.toml`, and they disagree on exactly
the inputs that feed the artifact hash:

| project | setting | effect on artifact identity |
|---|---|---|
| `HiveGPU` | `rustflags = ["-C","target-cpu=native"]` | different codegen → different hash for **every** dependency |
| `Transmutation`, `Vectorizer` | `rustflags = ["-A","warnings"]` | different `RUSTFLAGS` → different hash for every dependency |
| `ar-v3` | per-target `rustflags` incl. `target-cpu=x86-64-v3`, `-fuse-ld=lld` | same |
| `HivehubCloud/apps/api` | target `rustflags` with absolute `-L` paths | same, plus machine-specific |
| `Synap` | `[build] target-dir = "target"` | pins the target dir explicitly |
| `Tml/compiler/cranelift` | `[build] target-dir = "../../build/cranelift"` | pins the target dir elsewhere |
| `VecLite` | `[alias] xtask = ...` | benign |

**Impact.** This is decisive evidence against the shared-`CARGO_TARGET_DIR`
strategy for *this* fleet:

- Two projects already pin `build.target-dir` in project-local config, which
  **overrides** any global `$CARGO_HOME/config.toml` Targone might install. A
  global setting would silently not apply to them.
- Three or more projects set divergent `RUSTFLAGS`. Pointing them at one shared
  directory does not deduplicate `tokio` — it produces *two* `tokio` builds in
  one directory, plus lock contention between the projects, plus a single
  `cargo clean` that wipes everyone's cache at once.

Combined with F-014's ceiling of 16.3 GB, the shared-target-dir option is
expensive, risky, and small. `03-integration-mechanisms.md` develops this.

**Confidence: high** (files read directly, listed in `01-measurements.md`).
