# Analysis: Rust `target/` disk reduction — the full solution space

> 70 numbered findings (F-001…F-070) across 9 documents. Grounded in direct
> measurement of the primary dev machine (2026-08-18, Windows 10 / NTFS,
> cargo 1.97.1: **300.5 GB in 591,462 files across 18 target dirs**) plus
> source-verified research on Cargo internals, prior-art tools, and the
> upstream roadmap. Unverified claims are flagged inline with confidence
> levels.

## Files

| # | File | Theme | Findings |
|---|------|-------|----------|
| 01 | [Measurements](01-measurements.md) | What is actually on this disk — sizes, shapes, signals | F-001…F-010 |
| 02 | [Anatomy & growth](02-anatomy-and-growth.md) | Why `target/` grows; where the bytes live; Windows amplification | F-011…F-018 |
| 03 | [Integration mechanisms](03-integration-mechanisms.md) | Every way a crates.io module can hook in — with experiments | F-019…F-029 |
| 04 | [Prior art](04-prior-art.md) | cargo-sweep, kondo, sccache, & co. — source-verified | F-030…F-041 |
| 05 | [Policies](05-policies.md) | What to delete and how to decide — simulated on real data | F-042…F-049 |
| 06 | [Safety & concurrency](06-safety-and-concurrency.md) | Locks, deletion hazards, discovery, failure modes | F-050…F-057 |
| 07 | [Architecture recommendation](07-architecture-recommendation.md) | Beacon → registry → scheduler → sweeper | F-058…F-064 |
| 08 | [Execution plan](08-execution-plan.md) | Phased plan: spikes → report → sweep → recurrence → beacon | — |
| 09 | [Cargo upstream roadmap](09-cargo-upstream-roadmap.md) | What upstream ships/accepts/refuses; Targone's lifespan bounds | F-065…F-070 |

## The analysis in ten findings

1. **300.5 GB across 18 target dirs; 3 projects hold 91%** (F-001). Budgeting
   must be global, never a uniform per-project quota.
2. **`incremental/` is 65–74% of every large target dir** (F-002) — and a
   keep-newest-1-per-crate rule on it reclaims 96.9% of that pool with zero
   correctness cost (F-003). The biggest pool is the cheapest to reclaim.
3. **Only 4.1 GB of a 42.4 GB `deps/` is actual build input** (`.rlib`/`.rmeta`,
   F-004); ~90% is terminal output — test binaries and Windows PDBs nothing
   ever reads back. Worst case of deleting it: a re-link, not a re-compile.
4. **Age-based policies degenerate into `cargo clean`** (F-006/F-007/F-043):
   mtime records "when compiled", not "still in use", so any wall-clock
   threshold either does nothing or wipes a warm cache at a cliff. The correct
   primary rule is **identity-recency: keep the newest N per identity key**
   (F-044) — no cliff, no clock.
5. **Two headline policy stacks, simulated on the real data: 85.7% and 90.0%
   reclaimed** (F-045, F-049) with zero cold rebuilds.
6. **The niche is empty for structural reasons** (F-030): no crate prunes
   `target/` as a dependency; cargo-sweep (the closest tool) is unmaintained,
   skips `incremental/` entirely, keys on atime broken for 8 years, and takes
   no lock (F-031…F-035). `cargo-sweep` was *installed on this machine and
   never run* (F-010) — the gap is automatic invocation, not algorithms.
7. **Build-time cleanup is structurally unsafe** (F-019…F-023, F-036): a
   dependency's build script runs ~once ever, costs a full recompile if forced
   to re-run (measured 78 ms → 512 ms), cannot take the build lock Cargo
   already holds, and upstream has refused post-build hooks for a decade
   (F-067). Hence: the dependency is a **beacon** (registers the project),
   never the cleaner.
8. **Only the OS scheduler gives recurrence without touching builds** (F-025,
   F-028): a subcommand alone reintroduces "someone must remember to run it" —
   the exact failure being fixed.
9. **Safe deletion is a protocol, not a delete call** (F-050…F-057, F-061):
   take `.cargo-build-lock` exclusively per profile dir (externally possible
   with plain std file locks), metadata-only scanning, composite discovery
   (`CACHEDIR.TAG` alone misses 57% of the bytes here, F-055), Windows
   share-violation tolerance, fail-open on anything unrecognized.
10. **Upstream won't save us, and won't compete either** (F-065…F-068):
    Cargo's shipped GC covers 3% of the problem ($CARGO_HOME); whole-dir
    target GC is accepted but years out and registry-shaped like ours;
    the cross-workspace cache is a 2027+ story. The horizontal dimension is
    a measured distraction anyway — at most 5.4% here (F-014).

## Verdict

**Beacon → registry → scheduler → sweeper** (F-058): a `targone` crates.io
dependency that only registers the project; a durable registry at
`$CARGO_HOME/targone/`; an OS-scheduler entry as the sole source of
recurrence; and `cargo-targone` + `targone-core` doing lock-honest,
identity-recency sweeps ordered by reclaimable bytes under a global budget.
Execution is phased so deletion lands only after a read-only `report` phase
reproduces these measurements, and nothing deletes unsupervised before the
scheduler phase — see [08-execution-plan.md](08-execution-plan.md).
Target for this machine: **300.8 GB → ~43 GB (85.7%)**, warm builds preserved.
