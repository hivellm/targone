# 03 — Integration mechanisms: what a dependency crate can actually do

The problem statement asks for "a crate added as a dependency to each project"
that manages `target/` automatically. This file tests that premise empirically
rather than assuming it.

All findings here come from a purpose-built probe: a `targone-probe` crate with a
build script, consumed by a `consumer` crate, exercised under real `cargo`
1.97.1 on Windows. Probe sources are in the session scratchpad
(`scratchpad/probe/`).

---

## F-019 — A dependency's build script runs once per (target dir, profile), then never again

Probe: `cargo clean`, then six commands in sequence, counting build-script
executions.

| command | build-script executions (cumulative) |
|---|---:|
| `cargo check` (fresh target dir) | 1 |
| `cargo clippy` | 1 |
| `cargo test` | 1 |
| `cargo build` | 1 |
| `cargo doc` | 1 |
| `cargo build --release` | **2** |

The log shows exactly two invocations, one per profile:

```
ran: PROFILE=debug   OUT_DIR=...\consumer\target\debug\build\targone-probe-5dd5d07ed56908c9\out
ran: PROFILE=release OUT_DIR=...\consumer\target\release\build\targone-probe-b0464f298aca8336\out
```

A build script re-runs only when its fingerprint is invalidated. With no
`rerun-if-*` directives, Cargo's default is "re-run if any file in the package
changed" — and a package resolved from crates.io **never changes**. So for a
registry dependency the honest expected frequency is: *once per target directory
per profile, for the lifetime of that target directory.*

**Impact.** This alone disqualifies the naive reading of the goal. A dependency
crate cannot "run cleanup after each build", because it does not run after each
build — it runs approximately once, ever. Whatever the build script does must be
valuable when executed once and idempotent when executed again months later.
Registration is such a task. Periodic garbage collection is not.

**Confidence: high** (direct experiment).

---

## F-020 — Forcing per-build execution costs a full recompile of the crate and every dependent

`cargo::rerun-if-changed=<path that does not exist>` does make the script run on
every build. Cargo says so itself:

```
Dirty targone-probe v0.1.0: the file `...\__targone_always_rerun__` is missing
  Running `...\build\targone-probe-af05c6be876ebb3c\build-script-build`
  Running `rustc --crate-name targone_probe ... src\lib.rs ...`
Dirty consumer v0.1.0: the dependency `targone-probe` was rebuilt
  Running `rustc --crate-name consumer ... src\main.rs ...`
```

Measured cost on a two-crate project with nothing to do (median of 7 warm
builds, after 2 settling builds):

| variant | median warm build | dependent recompiled |
|---|---:|---:|
| **A** normal build script (no forced re-run) | **78 ms** | 0 / 7 |
| **B** always re-runs, stable output, does nothing | **512 ms** | **7 / 7** |
| **C** always re-runs + walks the whole target dir | 420 ms | 7 / 7 |
| **D** always re-runs + emits a changing `cargo::warning` | 403 ms | 7 / 7 |

Two things to note. First, the penalty is not the work — variant B does
*nothing* and is the slowest; the cost is the rebuild cascade, not the scan
(variant C's full directory walk is free by comparison). Second, emitting stable
output does not save you: **B is invalidated even though its output never
changes**, because Cargo marks the *package* dirty on the missing-file check
before it ever compares output.

**Impact.** This is the decisive finding against build-script-driven cleanup.
The cascade rule is "the dependency was rebuilt" → every transitive dependent
recompiles. On a trivial project that is 78 ms → 512 ms (6.5x). On Cortex —
where the dependent set of a foundational crate is the whole workspace plus ~90
binaries and integration tests — forcing this on every build would cost minutes
per build, to reclaim disk that a background pass could reclaim for free. A tool
that makes every build permanently slower in order to save disk has inverted its
own value proposition.

**Verdict: build.rs must never force itself to re-run.** It gets one shot; it
must use it to *register*, not to *clean*.

**Confidence: high** (direct measurement, Cargo's own diagnostic quoted).

---

## F-021 — `OUT_DIR` reliably locates the consumer's target root; nothing identifies the consumer itself

Full environment captured inside a dependency's build script:

```
CARGO_MANIFEST_DIR = ...\probe\targone-probe          <- the DEPENDENCY's dir, not the consumer's
CARGO_PKG_NAME     = targone-probe                    <- the DEPENDENCY's name
OUT_DIR            = ...\consumer\target\debug\build\targone-probe-77a4aea7c941e951\out
PROFILE            = debug        TARGET = x86_64-pc-windows-msvc     HOST = x86_64-pc-windows-msvc
RUSTUP_TOOLCHAIN   = stable-x86_64-pc-windows-msvc    NUM_JOBS = 32
CARGO_ENCODED_RUSTFLAGS = (empty)                     CARGO_HOME = C:\Users\Bolado\.cargo
```

Walking up from `OUT_DIR` looking for `CACHEDIR.TAG` found the target root in
**exactly 4 hops**, verified:

```
DISCOVERED_TARGET_ROOT = ...\consumer\target (after 4 hops up from OUT_DIR)
```

**Impact.** Split verdict, and both halves matter:

- **Target-dir discovery works and is robust.** `OUT_DIR` is always inside the
  consumer's target directory, wherever that has been relocated to
  (`CARGO_TARGET_DIR`, `build.target-dir`, a workspace root). Walking up to the
  nearest ancestor holding `CACHEDIR.TAG` is a correct, layout-independent
  algorithm — do not hardcode 4 hops, search for the marker.
- **Consumer identification does not work.** Every `CARGO_PKG_*` and
  `CARGO_MANIFEST_DIR` value describes *the dependency itself*. A build script
  cannot learn the name, version, or manifest path of the project being built.
  Any registry keyed on "project identity" must derive it from the target
  directory path, which is the only consumer-specific string available.

Also available and useful: `RUSTUP_TOOLCHAIN` and `TARGET` give the toolchain
identity for toolchain-keyed policies (F-009), and `CARGO_ENCODED_RUSTFLAGS`
exposes the flags that drive hash churn (F-011).

**Confidence: high** (direct measurement).

---

## F-022 — A build script blocks the build until every process it spawns exits — unless handle inheritance is disabled

Child process that sleeps 20 s, spawned four ways with `std::process::Command`
and all three stdio streams set to `Stdio::null()`:

| creation flags | build wall time |
|---|---:|
| (none — inherit) | **21.0 s** |
| `CREATE_NO_WINDOW` | **20.6 s** |
| `DETACHED_PROCESS` | **20.8 s** |
| `DETACHED_PROCESS \| CREATE_NEW_PROCESS_GROUP` | **20.7 s** |

Every naive strategy stalls the build for the child's full lifetime, because
Cargo reads the build script's stdout pipe until EOF and `std::process::Command`
spawns with `bInheritHandles = TRUE`, handing the grandchild a duplicate of that
pipe.

Bypassing `std` and calling `CreateProcessW` directly with
`bInheritHandles = FALSE`:

| approach | build wall time |
|---|---:|
| `std::process::Command` + `DETACHED_PROCESS` | **20.9 s** |
| raw `CreateProcessW`, `bInheritHandles = FALSE` | **0.9 s** |

...but in that run the child never completed its work: with inheritance off and
`DETACHED_PROCESS` set, the child received no valid standard handles and
`cmd.exe` failed to initialise. The correct form must open `NUL` explicitly,
mark only those three handles inheritable, and pass them via
`STARTF_USESTDHANDLES`.

Separately, `CREATE_BREAKAWAY_FROM_JOB` failed outright with
`Acesso negado (os error 5)` — job objects that forbid breakaway do exist in the
wild (CI agents, IDE-hosted terminals, sandboxes), so a design must not depend
on it.

**Impact.** "Build script fires a background agent" is *possible* but is a
platform-specific, easy-to-get-wrong piece of process plumbing — not the
one-liner it appears to be. Getting it wrong does not fail loudly; it silently
adds the agent's entire runtime to every build that triggers it, which for a
disk-cleanup pass could be minutes. If a spawn is used at all it must be:
raw `CreateProcessW` with explicit NUL handles on Windows; double-`fork` +
`setsid` + closing inherited descriptors on Unix; and in both cases guarded by a
watchdog that proves the build did not stall.

Handing the work to the OS scheduler (Task Scheduler / systemd user timer /
launchd) sidesteps the whole class of problem and should be preferred where it
is available.

**Confidence: high** for the blocking measurements; **high** for the
`bInheritHandles` fix; **medium** for the exact reason the raw child failed
(diagnosed, not instrumented).

---

## F-023 — Cargo has no post-build hook, and the build script is the only code a dependency gets to run

For completeness, the mechanisms by which a *dependency* can execute code during
someone else's build:

1. **`build.rs`** — runs at build time. Frequency and cascade behaviour as
   measured above.
2. **Proc-macro expansion** — a proc-macro crate runs arbitrary code inside
   `rustc` during compilation of its dependents. It re-runs whenever the
   dependent crate is compiled, so it does not have F-019's frequency problem.
   But it runs *inside the compiler process*, concurrently with dozens of other
   rustc invocations, holding no lock, with no knowledge of build completion —
   the worst possible place to delete files from the directory being written to.
   Rejected on safety grounds.
3. **Linker/`RUSTC_WRAPPER` shims** — not available to a dependency; they are
   configuration, not code (see F-026).
4. **Nothing else.** There is no post-build hook, no `cargo::` directive that
   requests one, and no crate-level lifecycle callback.

**Impact.** The design space for the "just add a dependency" requirement is a
single mechanism (`build.rs`) with a single well-timed invocation (F-019). The
requirement as literally stated cannot deliver recurring cleanup. It *can*
deliver zero-friction **enrolment** — which turns out to be the part that is
actually missing today (F-010).

**Confidence: high** for build.rs and proc-macros (experiment + Cargo
semantics); to be cross-checked against the prior-art survey in
`04-prior-art.md`.

---

## F-024 — Shared `CARGO_TARGET_DIR` is the highest-risk, lowest-payoff option for this fleet

Pointing all projects at one target directory is the classic answer to the
"horizontal" problem. Against the measured facts:

**Payoff ceiling: 16.3 GB (5.4%)** — the total cross-copy redundancy of every
`.rlib` on the machine (F-014), and that is the *perfect-sharing* number, not an
achievable one.

**Costs, all concrete:**

1. **Two projects already override it.** `Synap/.cargo/config.toml` pins
   `[build] target-dir = "target"` and `Tml/compiler/cranelift/.cargo/config.toml`
   pins `"../../build/cranelift"`. Project-local config beats global config, so a
   global setting silently does not apply to them (F-018).
2. **Divergent `RUSTFLAGS` prevent the sharing you paid for.** `HiveGPU` uses
   `target-cpu=native`; `Transmutation` and `Vectorizer` use `-A warnings`;
   `ar-v3` uses `target-cpu=x86-64-v3`. Different flags produce different hashes,
   so a shared directory does not deduplicate `tokio` — it accumulates *more*
   variants of `tokio` in one place.
3. **Locks serialise builds.** Cargo takes an exclusive lock per target
   directory (`target/<profile>/.cargo-build-lock` and siblings, F-050 and
   F-036). One shared directory means building project A
   blocks building project B. On a 32-job machine running several projects and a
   rust-analyzer per editor window, this is a serious regression.
4. **`cargo clean` becomes a fleet-wide weapon.** Any `cargo clean` in any
   project wipes the shared directory for all of them.
5. **It makes the vertical problem worse.** All the churn from F-011 now
   accumulates in one directory, and the tooling that could bound it (per-project
   heuristics, per-project quotas) loses the project boundary it needs.

**Impact.** Reject as a core strategy. The measured upside is 5.4%; the
mechanisms that deliver 85.7% (`05-policies.md`) do not require it, and it
actively degrades build concurrency. It may be worth revisiting **per project
group** — several small crates in one repo sharing a directory is safe when they
share flags — but never fleet-wide.

**Confidence: high** (payoff measured; every cost traced to a specific file or
Cargo behaviour).

---

## F-025 — A cargo subcommand has the right reach but reintroduces the exact failure this project exists to fix

A standalone `cargo targone` / `targone` binary can:

- see every project on the machine, including abandoned ones (F-017);
- run with no build in flight, so all the concurrency hazards vanish;
- take as long as it wants and use full parallelism;
- be tested and shipped independently of anyone's build.

Its single defect is that someone has to run it. On this machine `cargo-sweep`
is installed and unused (F-010) — the counterexample is already on disk.

**Impact.** The subcommand is the correct place to put the *engine* (policy,
scanning, deletion, locking) and the correct interface for dry-runs, reporting
and manual override. It is the wrong thing to rely on for *triggering*. This
splits the design cleanly: a library + binary that does the work, and a separate,
non-human trigger that decides when. See `07-architecture-recommendation.md`.

**Confidence: high.**

---

## F-026 — `RUSTC_WRAPPER` / sccache is a second cache, not a smaller one

`sccache` intercepts rustc invocations and serves cache hits from its own store.
It does not shrink `target/`: Cargo still requires every artifact to exist at its
expected path, so sccache *populates* `target/` exactly as rustc would and keeps
an **additional** copy in its own cache directory.

`Vectorizer/.cargo/config.toml` already documents (commented out) three ways to
enable it — evidence the team reached for it and did not adopt it.

**Impact.** sccache addresses *rebuild time* after a wipe, not disk usage. It is
complementary to Targone (it makes aggressive pruning cheaper to recover from)
and directly opposed to it on the specific axis of bytes on disk, since it adds a
second bounded store. Worth a documentation note; not part of the architecture.

**Confidence: high** for the mechanism; the exact cache-size behaviour to be
confirmed in `04-prior-art.md`.

---

## F-027 — A global `$CARGO_HOME/config.toml` is available, unoccupied, and useful for *policy*, not *triggering*

`C:\Users\Bolado\.cargo\config.toml` does not exist. A global config could set,
fleet-wide and overridable per project:

```toml
[profile.dev]
debug = "line-tables-only"   # or 0
incremental = true
[build]
# NOT target-dir — see F-024
```

Cortex already sets `debug = "line-tables-only"` locally and still carries
25.3 GB of PDBs (F-004), so this is a mitigation, not a fix. `debug = 0` would
be a genuine reduction but destroys debuggability and backtraces — a policy
decision for the user, not a default a disk tool should impose.

**Impact.** Configuration tuning is a legitimate, zero-risk *supplementary*
lever and belongs in Targone's advice output ("your dev profile emits full debug
info; here is what that costs you"). It cannot be the mechanism, because it
cannot delete anything that already exists.

**Confidence: high.**

---

## F-028 — Only OS-level scheduling gives recurring execution without touching builds

The trigger mechanisms that can fire without a human and without slowing a
build:

| trigger | Windows | Linux | macOS | notes |
|---|---|---|---|---|
| OS scheduler | Task Scheduler | systemd user timer / cron | launchd | survives reboot; no build impact; needs one-time registration |
| **PATH shim in front of `cargo`** | yes | yes | yes | fires once per cargo invocation, **after** the build exits; no scheduler needed; intercepts every call, so a bug breaks all builds |
| long-lived daemon | service | user unit | LaunchAgent | heavier; a resident process users must trust |
| git hooks (`post-checkout`, `post-merge`) | yes | yes | yes | per-repo, not installed by a dependency, easily clobbered |
| shell hook (`PROMPT_COMMAND`, `precmd`) | n/a | yes | yes | not portable to the primary Windows machine |
| next build's build script (F-019) | rare | rare | rare | fires roughly once per target dir |

The PATH-shim row is not hypothetical — `cargo-overstay` ships it (F-040): you
put a shim ahead of the real `cargo` on `PATH`, it forwards the invocation, and a
background worker cleans after Cargo exits. It has the best possible timing
(immediately after a build, when the target dir is warm, the locks are free, and
the live set was just proven) and it requires no per-project change whatsoever.
Its cost is blast radius: every `cargo` call on the machine now routes through
third-party code, and a failure mode there is indistinguishable from a broken
toolchain.

**Impact.** Recurring, build-independent execution requires a registered
scheduler entry. Registration is a one-time act — precisely what a build script
*can* reliably do with its single guaranteed invocation (F-019). That
complementarity is the seam the architecture should be built on: **`build.rs`
registers; the scheduler triggers; the binary executes.**

**Confidence: high** for the mechanism matrix; the scheduler-registration
ergonomics (permissions, per-user vs per-machine) need a spike — see
`08-execution-plan.md`.

---

## F-029 — Mechanism comparison

Scored against the problem statement's four success criteria.

| mechanism | recurring? | build cost | reaches abandoned projects | safe vs concurrent build | max reclaim |
|---|---|---|---|---|---|
| `build.rs` cleanup, forced re-run | yes | **fatal** (F-020) | no | poor (runs mid-build) | 85% |
| `build.rs` cleanup, natural frequency | ~once ever | zero | no | poor | ~85% once |
| **`build.rs` registration only** | ~once ever | **zero** | **yes, permanently** | n/a | n/a (enables) |
| proc-macro | per compile | high | no | **unsafe** | — |
| cargo subcommand, manual | no (F-010) | zero | yes | **good** | 85% |
| cargo subcommand + OS scheduler | **yes** | zero | **yes** | **good** | **85%** |
| PATH shim ahead of `cargo` | **yes** | zero (runs after exit) | no | **good** | **85%** |
| shared `CARGO_TARGET_DIR` | n/a | lock contention | no | poor | 5.4% (F-024) |
| `RUSTC_WRAPPER` / sccache | n/a | improves | no | good | **negative** (F-026) |
| global config profile tuning | n/a | zero | no | good | partial, no retro effect |

**Impact.** Exactly one row satisfies every criterion, and it is a *composition*:
a build-script beacon that enrols the project once, an OS scheduler that provides
recurrence, and a standalone binary that performs the work outside any build.
No single mechanism suffices, and the "just a dependency" framing is best read as
a statement about *adoption friction* — which the beacon preserves in full —
rather than about where the code runs.

**Confidence: high.**
