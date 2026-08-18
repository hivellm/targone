# 07 — Recommended architecture

**Recommendation in one sentence:** build a standalone sweeper whose policy is
*"keep the newest identity, delete its predecessors"*, run it from the OS
scheduler while holding Cargo's build lock, and ship the crates.io dependency as
a **beacon that only registers the project** — never as the thing that cleans.

This reclaims a measured **257.7 GB of 300.8 GB (85.7%)** with zero cold
rebuilds, zero build-time cost, and no shared target directory.

---

## F-058 — Architecture: beacon → registry → scheduler → sweeper

Four parts, each doing the one thing it is structurally capable of doing.

```
  ┌─ targone (crates.io dependency) ────────────────────────────┐
  │  build.rs, runs ~once per target dir per profile (F-019)    │
  │  • walk up from OUT_DIR to CACHEDIR.TAG  → target root      │
  │  • append {root, toolchain, rustflags, profile, first_seen} │
  │  • ensure a scheduler entry exists                          │
  │  • NEVER deletes. NEVER forces re-run. NEVER spawns.        │
  └───────────────────────┬─────────────────────────────────────┘
                          │ appends
  ┌───────────────────────▼─────────────────────────────────────┐
  │  registry  —  $CARGO_HOME/targone/registry.jsonl            │
  │  append-only, durable; survives the project going dormant    │
  └───────────────────────┬─────────────────────────────────────┘
                          │ read by
  ┌───────────────────────▼─────────────────────────────────────┐
  │  scheduler entry  —  Task Scheduler / systemd timer/launchd │
  │  fires on idle, daily; the ONLY source of recurrence (F-028)│
  └───────────────────────┬─────────────────────────────────────┘
                          │ invokes
  ┌───────────────────────▼─────────────────────────────────────┐
  │  cargo-targone (binary)  +  targone-core (library)          │
  │  discover → order by reclaimable bytes → per profile dir:   │
  │    acquire .cargo-build-lock → classify → delete → release  │
  └─────────────────────────────────────────────────────────────┘
```

Crates:

| crate | role | why separate |
|---|---|---|
| `targone` | the dependency; build.rs beacon only, no runtime API | must stay tiny — it is compiled into everyone's build (F-020) |
| `targone-core` | scan, classify, lock, delete | testable without a build; reusable by other tools |
| `cargo-targone` | CLI / `cargo targone …` | the engine, the dry-run UI, the manual override (F-025) |

**Why this split and not the obvious one.** The problem statement's literal
reading — a dependency that cleans — is closed off by three independent facts:
a dependency's build script runs about once ever (F-019); forcing it to run more
often recompiles the crate and everything downstream (F-020, measured 78 ms →
512 ms); and Cargo holds the build lock exclusively for the whole build, so a
build script *cannot* acquire the lock that would make deletion safe (F-036).
The beacon preserves what the requirement was actually after — **adoption by
`cargo add` and nothing else** — while moving the work to the only place it can
be done safely.

**Confidence: high** — every constraint behind this shape is measured (F-019,
F-020, F-022, F-036, F-050) rather than assumed.

---

## F-059 — The beacon is not load-bearing, and the plan should say so

A filesystem scan for `CACHEDIR.TAG` under a few configured roots found **all 18**
target directories on this machine with no project changes at all (F-001,
F-055). The beacon adds four things a scan cannot get:

1. target dirs **outside** the scanned roots (relocated via `CARGO_TARGET_DIR`
   or `build.target-dir` — two projects here already do this, F-018);
2. the **toolchain and RUSTFLAGS** that produced the directory (F-021), which a
   scan can only partially recover;
3. enrolment **without the user configuring scan roots**;
4. a durable record that outlives the project going dormant (F-017).

None of those is worth 85% of the value. The scan gets that on day one.

**Impact.** Sequence the work accordingly: **the sweeper and the scanner are
Phase 1 and deliver the entire measured win; the beacon is Phase 3 and is an
adoption-ergonomics feature.** Building the beacon first would put the riskiest,
least valuable component on the critical path, and would tempt the design back
toward build-time cleanup. If the beacon never ships, Targone still solves the
problem.

**Confidence: high** (the scan was performed and found everything).

---

## F-060 — Determine liveness by identity-recency, not by decoding Cargo's hashes

Three candidate liveness signals, and why the ranking is what it is:

| signal | stability | cost | verdict |
|---|---|---|---|
| **newest hash per identity** (F-044) | high — compares hashes only to each other, never interprets them | one metadata walk | **primary** |
| `compiler-artifact` JSON (`--message-format=json`) | high — documented, stable public interface (F-040) | requires running a build | **opportunistic corroboration** |
| decoding Cargo's `rustc` hash from `.fingerprint/*.json` (F-032) | **low** — the algorithm already changed once at Rust 1.85 | cheap | **safety net only** |
| `atime` (F-047, F-033) | **very low** — off on Windows, `relatime`/`noatime` on Linux, and Cargo does not touch it on cache reuse | free | **may only spare, never condemn** |

The primary rule needs no knowledge of what a hash *means*. It groups
`deps/libtokio-<hash>.rlib` and `incremental/tokio-<disambiguator>/` by the name
part, sorts by mtime, keeps the newest, deletes the rest. That is immune to
Cargo changing its hashing (F-032) and largely immune to it changing its layout
(F-041), because it only assumes "artifacts of one logical unit share a name
stem and differ by a suffix".

**Impact.** This is the design decision that most distinguishes Targone from
`cargo-sweep`. cargo-sweep is coupled to two Cargo internals (the rustc hash
algorithm and the fingerprint JSON schema) and to an OS behaviour that does not
hold (`atime`), which is why its time-based modes have been broken for eight
years (F-033). Identity-recency is coupled to none of them.

Adopt cargo-sweep's one genuinely good invariant unchanged: **fail open.** Any
artifact that cannot be positively classified as superseded is kept.

**Confidence: high** for the ranking; the `compiler-artifact` corroboration path
is **medium** (design, not yet prototyped).

---

## F-061 — The sweep protocol

Per target directory, per profile directory, in this order:

1. **Verify** `CACHEDIR.TAG` at the target root (F-055). Refuse otherwise —
   30 of 48 name-matched directories on this machine were LLVM/GCC sources.
2. **Refuse network filesystems.** Cargo silently skips locking on NFS (F-036);
   with no lock there is no safe sweep.
3. **Acquire** `target/<profile>/.cargo-build-lock` exclusively — via
   `cargo-util` if possible, so the semantics match Cargo's exactly. Do not
   merely test it (F-054). On Unix this must be `flock`, which is advisory:
   holding it is what excludes Cargo (F-050).
4. **Classify** with metadata only — never open an artifact (F-056).
5. **Delete** per file, tolerating `ERROR_SHARING_VIOLATION` as a normal skip;
   remove a directory only once it is verifiably empty (F-053). Use the
   `remove_dir_all` crate for Windows-hardened deletion (F-037).
6. **Release**, then move to the next profile directory.

Bounding rules that follow from the measurements:

- **One profile directory per lock acquisition.** Six rust-analyzer processes are
  running right now (F-052); holding a lock across a 172 GB sweep would present
  as the IDE hanging. Sweep in bounded units and release between them.
- **Order directories by reclaimable bytes, descending.** Three projects hold 91%
  (F-001); processing in that order means an interrupted sweep still captured
  nearly all of the win.
- **Compute any size budget over reclaimable bytes only**, never over total size
  — the bug that makes `cargo-sweep --maxsize` degenerate into `cargo clean`
  (F-034, F-048).
- **Dry-run by default on first run**, with a written report. The asset being
  protected is the user's trust; one corrupted build spends it all (F-051).

**Confidence: high** for steps 1-6 (each traced to a measurement);
**medium** for the bounding constants, which need a load test (see `08`).

---

## F-062 — Anti-requirements: things Targone must never do

Each is here because a measurement showed it is harmful, and each is a plausible
thing an implementer would otherwise reach for.

| # | never | because |
|---|---|---|
| 1 | force a build script to re-run | recompiles the crate and every dependent, permanently and **silently** (F-020) |
| 2 | delete anything from a build script | cannot hold the lock it would need — it runs inside the holder (F-036) |
| 3 | spawn a background process from a build script naively | blocks the build for the child's full lifetime, all four strategies (F-022) |
| 4 | set a fleet-wide shared `CARGO_TARGET_DIR` | ceiling of 16.3 GB (5.4%), overridden by two projects already, serialises builds (F-014, F-018, F-024) |
| 5 | use absolute age as a per-artifact criterion | degenerates to `cargo clean` on any dormant project (F-007, F-043) |
| 6 | condemn an artifact because `atime` is old | on `noatime` that rule silently means "delete everything" (F-047, F-033) |
| 7 | discover target dirs by directory name | would have deleted LLVM/GCC sources here (F-055) |
| 8 | read artifact contents while scanning | destroys the `atime` signal for itself and every other tool (F-056) |
| 9 | recursively delete a directory as one operation | fails halfway on Windows, leaving a corrupt incremental session (F-053) |
| 10 | run in CI | CI containers are ephemeral; the tool's cost is pure loss there |

**Impact.** Items 1 and 3 deserve emphasis because they fail *silently*: the user
gets a slower machine and never connects it to a disk tool installed months
earlier. A disk tool that quietly taxes every build has negative net value.

**Confidence: high** (each traced to a numbered measurement).

---

## F-063 — What this does and does not achieve against the stated success criteria

| criterion | verdict | evidence |
|---|---|---|
| 1. aggregate usage drops an order of magnitude and **stays** bounded | **drop: yes** — 300.8 → 43.1 GB (7.0x), or 30.1 GB (10.0x) with tier 5. **Stays bounded: by construction** — the residual is the live set, which does not grow with churn | F-045 |
| 2. no manual per-project intervention after adoption | **yes**, via the scheduler; and with the scanner, no per-project adoption is needed at all | F-028, F-059 |
| 3. build-time cost negligible; warm builds preserved | **yes, and stronger than asked** — build-time cost is exactly zero (nothing runs during a build), and no warm artifact is deleted | F-042, F-058 |
| 4. safe by default; cannot break an in-progress build | **conditional** — safe *if* the lock protocol is implemented correctly; no existing tool does this, so it is unproven in the field | F-061, F-035 |

The honest weak point is criterion 4. It is the one requirement that cannot be
established by analysis, only by a load test under concurrent builds — which is
why `08-execution-plan.md` puts that test before any deletion feature.

Two premises of the problem statement were **not** confirmed by measurement and
should be revised:

- *"per-profile duplication"* — `release/` is a rounding error here (F-013).
- *"no cross-project sharing… ten projects compile the same tokio ten times"* —
  true, and worth **0.69 GB**. The whole cross-project redundancy is 16.3 GB,
  5.4% of the problem (F-014). The vertical dimension is ~95% of it.

**Confidence: high** for criteria 1-3; **medium** for 4 (design is sound;
implementation is where the risk lives).

---

## F-064 — Open questions requiring a spike, not more analysis

1. **Lock behaviour under real concurrency.** Does holding `.cargo-build-lock`
   for a bounded sweep degrade rust-analyzer perceptibly? What is the largest
   sweep unit that stays imperceptible? (F-052, F-054)
2. **Unix lock semantics.** F-050 was measured on Windows only. `flock`
   advisory-lock behaviour and `unlink`-of-open-file need equivalent probes on
   Linux and macOS. (F-053)
3. **Scheduler registration ergonomics.** Task Scheduler / systemd user timer /
   launchd registration from a build script: permissions required, per-user vs
   per-machine, behaviour when the user lacks rights, and how to avoid duplicate
   entries across many projects. (F-028)
4. **The `.fingerprint` contradiction.** cargo-mark-sweep claims some units have
   fingerprint hashes appearing in no artifact filename; cargo-sweep deletes from
   `.fingerprint/` regardless. My measurement found a 198-entry excess consistent
   with the claim. Resolve before touching `.fingerprint/` — the safe interim
   position is **do not delete from `.fingerprint/` at all**, which costs 0.01 GB.
   (F-005, F-040)
5. **Layout drift.** `build.build-dir` and `-Zbuild-dir-new-layout` restructure
   the directories every tool hardcodes. What is the minimum layout assumption
   Targone can make, and how does it detect an unrecognised layout and fail
   closed? (F-041)
6. **Incremental-dir identity parsing.** The primary policy groups
   `incremental/<crate>-<disambiguator>/` by name stem. The disambiguator is
   rustc's, not Cargo's, and its format is not a stable interface. Confirm the
   grouping rule is robust, including for crate names containing `-`. (F-003)

**Confidence: high** that these are the right six; each is a decision the
analysis cannot settle from the outside.
