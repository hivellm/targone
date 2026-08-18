# 05 — Cleanup policies: what "stale" should mean

The problem statement asks whether staleness should be age-based,
toolchain-based, access-based or budget-based. The measurements answer this more
sharply than a discussion could: **the axis that matters is not time, it is
identity and role.**

---

## F-042 — The cost of deleting each artifact class, measured

Four controlled deletions on a built project, each followed by a rebuild:

| deleted | units recompiled | rebuild time | artifact restored? |
|---|---:|---:|---|
| baseline (nothing) | 0 | 77 ms | — |
| **entire `incremental/`** | **0** | **100 ms** | no (regenerated on next real change) |
| **a `.pdb` from `deps/`** | **0** | **75 ms** | **no — Cargo never noticed** |
| an `.rlib` from `deps/` | 2 | 403 ms | yes |
| a stale `.fingerprint/` dir | 0 | 105 ms | n/a |

**Impact.** This is the cost model the policy engine needs, and it is remarkably
lopsided:

- **`incremental/` is free to delete.** Cargo does not fingerprint it. Removing
  all of it left every unit fresh — zero recompilation. The real cost is
  deferred and bounded: the *next* edit to a local crate compiles
  non-incrementally once. Against 201.8 GB machine-wide (F-002), this is the
  best trade available anywhere in the system.
- **`.pdb` is free to delete and Cargo does not even restore it.** It is outside
  the fingerprint graph entirely. 45.1 GB machine-wide, zero rebuild cost, and
  the only consequence is that a debugger cannot symbolise that particular
  binary until it is relinked for some other reason.
- **`.rlib`/`.rmeta` are the load-bearing artifacts.** Deleting one forces
  recompilation of that unit and everything downstream. These are exactly the
  4.1 GB (of 42.4 GB in Cortex) identified in F-004 — small, and worth
  protecting absolutely.

The policy therefore writes itself: **delete by role first, and only then worry
about time.**

**Confidence: high** (direct experiment; the `incremental/` result is the single
most consequential measurement in this analysis).

---

## F-043 — Age-based policy: rejected as a primary rule

Already shown in F-007: on a directory last built 9 days ago, an `age > 7d` rule
frees 100% and an `age > 14d` rule frees 0.2%. There is no threshold that is
simultaneously safe for an active project and effective on a dormant one,
because the quantity being thresholded (wall-clock age) is a property of *when
you last worked*, not of *what is redundant*.

Age remains useful in exactly two narrow roles:

1. **Relative to the target dir's own newest build.** "Delete units more than N
   builds / N days older than the most recent build *in this directory*" has no
   cliff: a project untouched for a year is compared against its own last build,
   not against today.
2. **As a whole-directory dormancy trigger.** "This target dir has not been built
   in 90 days → it is a candidate for full reclamation" is a legitimate and
   valuable rule (F-017's abandoned projects), and it is precisely the case where
   a cold rebuild costs nothing because nobody is building it.

**Impact.** Absolute age must not gate individual artifacts. It may gate whole
directories.

**Confidence: high** (simulation in F-007).

---

## F-044 — Identity-based policy ("keep newest N per key") is the correct primary rule

The waste has the shape of *many identities for one logical thing* (F-011).
A policy shaped the same way matches it exactly. Simulated machine-wide:

| pool | total | policy | freed | % |
|---|---:|---|---:|---:|
| `incremental/` | 201.8 GB | keep newest **1** dir per crate name | **193.1 GB** | 95.7% |
| `deps/` | 88.4 GB | keep only newest-hash artifacts per base name | **61.9 GB** | 70.0% |
| `build/` | 3.2 GB | keep newest dir per package name | 2.6 GB | 81.3% |

Per-project detail for the three largest:

| project | total | incremental (freed) | deps (freed) | all `.pdb` |
|---|---:|---:|---:|---:|
| Cortex | 172.0 | 127.0 (**123.1**) | 42.5 (**30.3**) | 25.3 |
| Thunder | 56.7 | 38.1 (**36.9**) | 17.8 (**14.8**) | 7.5 |
| ar-v3-dashboard | 45.1 | 30.3 (**27.9**) | 14.0 (**10.2**) | 8.0 |
| Nexus/sdks/rust | 7.7 | 3.0 (2.7) | 3.8 (3.0) | 1.8 |
| ar-database-sync | 8.6 | 2.3 (1.8) | 3.3 (1.1) | 1.1 |

**Impact.** The key insight is that "newest per identity" needs no clock, no
configuration, and no guess about the user's habits. It is self-calibrating: an
active project keeps its live set and sheds its history; a dormant project keeps
its last-known-good set and sheds its history; neither is ever left cold. It also
degrades gracefully — `keep newest 2` or `3` still frees 95.3% / 94.2% of
`incremental/` (F-003), so the knob is available without being load-bearing.

**Confidence: high** (simulation over the full measured population).

---

## F-045 — Two headline policies: 85.7% and 90.0% reclaimed

Composing the rules above across all 15 non-empty target directories:

```
total target/            :    300.8 GB
  incremental/           :    201.8 GB   (keep newest 1/crate frees 193.1)
  deps/                  :     88.4 GB   (drop stale hashes frees    61.9)
  build/                 :      3.2 GB   (keep newest 1/pkg frees     2.6)
  .pdb everywhere        :     45.1 GB   (of which stale-hash        32.1)

POLICY A (stale-only)     : free 257.7 GB = 85.7% of total; residual 43.1 GB
POLICY B (A + drop all pdb): free 270.7 GB = 90.0% of total; residual 30.1 GB
```

- **Policy A — conservative.** Deletes only superseded identities. Every artifact
  the most recent build could reuse is preserved. Worst case after A: the next
  local edit compiles non-incrementally once. **300.8 GB → 43.1 GB (7.0x).**
- **Policy B — A plus all PDBs**, including current ones. Free per F-042 (Cargo
  does not restore or notice them); costs only symbolised debugging of binaries
  built before the sweep. **300.8 GB → 30.1 GB (10.0x).**

**Impact.** The problem statement's success criterion 1 — "aggregate usage drops
by an order of magnitude and *stays* bounded" — is met exactly by Policy B and
nearly by Policy A, **without any cross-project sharing, without a shared target
directory, and without a single cold rebuild.** Criterion 3 (warm builds
preserved) is satisfied by construction, since the deleted set is by definition
what the newest build does not reference.

**Confidence: high** for the freed figures (measured and simulated per-file);
**medium** for "*stays* bounded" — that depends on the trigger cadence, addressed
in `07-architecture-recommendation.md`.

---

## F-046 — Toolchain-keyed staleness is real but subsumed

Seven toolchains are installed (F-009): stable, nightly, nightly-2025-01-01,
nightly-2026-02-27, 1.87, 1.88, 1.93.1. Artifacts built by a toolchain that is no
longer installed can never be reused — they are unambiguously dead, which makes
this the *safest* possible deletion criterion, and it is what `cargo-sweep
--installed` is built around.

`target/.rustc_info.json` (2,428 bytes at the target root) records the compiler
identity for the directory. Note it describes the *directory's most recent*
compiler, not per-artifact provenance — per-artifact toolchain attribution
requires reading each `.fingerprint/*/lib-*.json`.

**Impact.** Worth implementing as a **safety net and a fast path**, not as the
primary rule: any artifact attributable to an uninstalled toolchain can be
deleted with no analysis at all. But it is largely subsumed by F-044 — a
toolchain change produces new hashes, so those artifacts are already "not the
newest identity". Its unique value is the reverse direction: it justifies
deleting artifacts *even when they are the newest of their identity*, because
the compiler that made them is gone.

**Confidence: high** for the mechanism; **low** for the incremental byte savings
over F-044 (not separately simulated — they overlap heavily).

---

## F-047 — Access-based (`atime`) policy: a good optimiser, an unsafe primitive

`atime` is live on this machine and cleanly separates consumed inputs from
terminal outputs (F-008): in Cortex's `deps/`, 1,531 files (1.8 GB) were re-read
after being written, and 7,702 files (40.6 GB) never were. The re-read set is
almost exactly the `.rlib`/`.rmeta` set (F-004).

But it cannot be relied on:

- Windows `DisableLastAccess` is system-managed and can be disabled outright;
  when enabled, updates have ~1 hour granularity.
- Linux defaults to `relatime` (update only if atime < mtime or >24 h old) — good
  enough here — but `noatime` is common on SSD-tuned systems and in containers,
  and yields *nothing*.
- Any tool that reads artifact contents (including a naive implementation of
  Targone itself) corrupts the signal. Metadata-only enumeration does not, which
  is why every scan in this analysis used `FileInfo.Length` rather than opening
  files.

**Impact.** Use `atime` as a **confirmation signal that can only spare files,
never condemn them**: "this artifact is the newest of its identity *and* was read
recently → definitely keep". Never as "not read in N days → delete", because on a
`noatime` filesystem that rule silently becomes "delete everything". The
implementation must detect atime availability at runtime (write a temp file, read
it, check whether atime moved) and degrade to identity-only policy when it is
absent.

**Confidence: high** for the measurement and the caveats.

---

## F-048 — Size-budget policy: necessary as a trigger, wrong as a rule

The distribution is extremely skewed — three projects hold 91% (F-001). A
uniform per-project budget is therefore either useless (15 projects are already
under any sane cap) or destructive (3 projects would be cut to the bone
regardless of what is live).

The useful formulation inverts it: a **global budget with per-directory
prioritisation**. "Keep total target/ usage under X GB; when over, apply Policy A
to directories in descending order of reclaimable bytes until under." That
targets effort where the bytes are, needs no per-project tuning, and gives the
user exactly one number to set.

**Impact.** Budget belongs in the *scheduler* layer as a trigger and an ordering
function, not in the *policy* layer as a deletion criterion. The deletion
criterion stays "superseded identity" (F-044); the budget only decides how many
directories to process and how hard.

**Confidence: high** for the skew; **medium** for the specific formulation
(not simulated — it is a scheduling heuristic, not a measurement).

---

## F-049 — Recommended policy stack

Ordered from safest to most aggressive. Each tier is independently switchable;
the default is tiers 1-4.

| # | tier | rule | freed (machine-wide) | rebuild cost |
|---|---|---|---:|---|
| 1 | **dead identities, incremental** | keep newest 1 dir per crate name in `incremental/` | 193.1 GB | none (F-042) |
| 2 | **dead identities, deps** | in `deps/`, keep only artifacts of the newest hash per base name | 61.9 GB | none — superseded by definition |
| 3 | **dead identities, build** | keep newest dir per package in `build/` | 2.6 GB | none |
| 4 | **orphan fingerprints** | drop `.fingerprint/` dirs with no surviving artifact | ~0.01 GB | none |
| 5 | **all PDBs** (opt-in) | delete every `.pdb` under `target/` | +13.0 GB | none; loses symbolisation |
| 6 | **dormant directories** (opt-in) | target dirs unbuilt for > N days → full reclaim | remainder | cold rebuild, if ever |
| 7 | **uninstalled toolchains** | artifacts from toolchains no longer present | overlaps 1-3 | none |

Tiers 1-4 = **Policy A, 257.7 GB, 85.7%**. Adding tier 5 = **Policy B, 270.7 GB,
90.0%**.

Guard rails that apply to every tier:

- Never touch a target directory whose Cargo locks are held (F-050).
- Never delete the newest identity of anything except under tier 5 (PDBs, which
  Cargo does not track) or tier 6 (explicitly dormant).
- Never follow symlinks/junctions or cross a filesystem boundary.
- Only operate inside a directory carrying `CACHEDIR.TAG` (F-017).
- Report before deleting; dry-run must be the default for the first run.

**Confidence: high** for tiers 1-5 (each measured); **medium** for tier 6-7
sizing.
