# 04 — Integration mechanisms: every way a crates.io module can hook in

The product requirement is "add a crate to each project and the problem
manages itself." This file maps every mechanism a published crate can use,
with a verdict on each.

## A. `build.rs` as the GC engine — **rejected**

The obvious reading of the requirement ("dependency whose build script
cleans the target dir") fails on seven independent grounds, all verified
against Cargo 1.97 source or open issues:

1. **No target-dir path.** Build scripts get `OUT_DIR` but no
   `CARGO_TARGET_DIR`; the request (#9661) is `S-propose-close`, refused on
   design grounds. Walking up from `OUT_DIR` is now actively wrong: since
   1.91 `OUT_DIR` is under *build-dir*, which may be nowhere near `target/`.
2. **Doesn't fire when needed.** On a warm (fresh) build Cargo replays the
   cached script output and never executes the script. The trigger fires
   only on cold/dirty builds — exactly when deletion is most dangerous —
   and never on the idle projects that need cleanup most.
3. **Fires when it must not.** Build scripts run under `cargo check`,
   `cargo doc`, docs.rs (read-only FS, no network), and `cargo publish`
   verify — which since Rust 1.28 hard-errors if a script modifies files
   outside `OUT_DIR` ("Source directory was modified by build.rs").
4. **Deadlock.** The parent Cargo holds `.cargo-build-lock` exclusively for
   the whole build; a script that tries to take it (or to invoke `cargo`)
   deadlocks (#8938). Deleting *without* the lock races the compiler that
   spawned us.
5. **Detached children hang the build.** Cargo waits for EOF on the script's
   piped stdout/stderr, not for exit — a spawned daemon inheriting stdio
   blocks the build forever. (Mitigable with null stdio, but see 6.)
   On Windows, Cargo's Job Object kills children on Ctrl-C.
6. **It reads as malware.** The 2023 crates.io malware postmortem was
   build-scripts-doing-filesystem/network-things; policy (2026-02) is
   RustSec advisories on removal. A dependency whose build script scans and
   deletes files across the user's disk is behaviorally indistinguishable
   from that pattern — a trust and adoption killer for a public crate.
7. **Cargo Book policy**, verbatim: "Scripts should not modify any files
   outside of [`OUT_DIR`]."

## B. `build.rs` as a **registration ping** — accepted

Everything in A poisons *deletion*, none of it poisons a **~1ms append-only
side-effect**: write `{manifest_path, workspace_root, timestamp, profile,
toolchain}` to a machine-global registry (`~/.targone/registry.jsonl` or
platform-equivalent via `directories`). Assessment against the same list:

- Needs no target-dir path (the *engine* resolves target dirs later via
  `cargo metadata` / config, outside any build).
- Warm builds not firing is acceptable: registration is idempotent and any
  cold/dirty build refreshes it; a project with zero dirty builds for weeks
  is *precisely* the "idle" signal we want.
- Must be **fail-silent** (never break a build), **no-op** under
  `DOCS_RS`/`CARGO_PRIMARY_PACKAGE` absence rules, on read-only FS, and in
  CI when undesired (env opt-out `TARGONE_DISABLE=1`).
- No child processes, no stdio games, no locks.
- Transparent and documented: writes one line to one well-known file; the
  antithesis of the malware pattern.

This is the mechanism that honors "add a crate to each project": the crate
is the **adoption interface and activity beacon**, not the engine.

## C. Cargo subcommand (`cargo targone`) — accepted (the engine)

The officially recommended extension point (any `cargo-<name>` binary on
PATH; the path RFC 1777's rejection explicitly pointed to). Runs *outside*
builds: full filesystem access, can take Cargo's real locks, can run
`cargo metadata`/`cargo build --message-format=json`, can be scheduled.
Installed once per machine (`cargo install cargo-targone` / `cargo binstall`),
not per project — the per-project piece stays the B crate.

## D. Scheduled background execution — accepted (delivery of C)

The engine must run without being remembered — that's the product's whole
point. Options per platform, all invoking the same `cargo targone gc`:

- **Windows** (primary): Task Scheduler entry, created by `cargo targone
  setup` (daily / on-idle). No service, no resident daemon.
- Linux: systemd user timer; macOS: launchd agent.
- Fallback: opportunistic self-throttled run piggybacked on any manual
  `cargo targone` invocation (like Cargo's own `cache.auto-clean-frequency
  = "1 day"` model).

A resident daemon is rejected: adds failure modes, and a timer covers the
cadence (hourly/daily is more than enough for a GC).

## E. `cargo` wrapper / PATH shim (cargo-overstay model) — optional, not core

A shim wrapping `cargo` sees every build start/end and could GC right after
each build with perfect timing. Rejected as the *primary* mechanism:
intrusive to install, fragile across rustup shims/IDEs/CI, and per-machine
rather than per-project. Worth keeping as an opt-in later for users who want
build-triggered GC. (`RUSTC_WRAPPER` is similarly rejected: it intercepts
compiler calls mid-build — the wrong side of the lock — and conflicts with
sccache users.)

## F. Configuration lever: `build.build-dir` — accepted (opt-in migration)

Not a hook but the highest-leverage stable lever (1.91+): point every
project's build-dir at one central location,
`build-dir = "{cargo-cache-home}/build/{workspace-path-hash}"`:

- ~90% of bytes leave per-project `target/` (cargo itself: 4.2 GB → 415 MB);
  what remains next to the project is small.
- `{workspace-path-hash}` keeps workspaces isolated — sidesteps the shared-
  target-dir correctness bugs (#12516 same-name path deps silently sharing
  artifacts; #7740 same-name binaries colliding) and the whole-dir lock
  contention (#4282).
- Centralization makes the engine's job trivial: one root to size-budget,
  and orphan detection is "registry says this workspace no longer exists".
- Caveat: build-dir and target-dir on different volumes degrades uplift
  from hardlink to copy — keep them on the same volume.

`cargo targone setup --central-build-dir` edits the user's global
`~/.cargo/config.toml` (with consent); projects can also carry it in
`.cargo/config.toml` per-project.

## G. Shared `CARGO_TARGET_DIR` across projects — rejected

The classic advice, and it's a trap: feature unification is per-workspace so
"same dependency" rarely hashes identically across projects; on stable the
whole dir is one lock (concurrent builds of different projects serialize);
`cargo clean` nukes everyone; and it triggers the #12516/#7740 correctness
bugs. Upstream's own book recommends sccache instead of this. F gives the
blast-radius benefits without the bugs.

## Verdict table

| Mechanism | Role in Targone |
|---|---|
| A. build.rs deletes | ✖ rejected — unsafe, unreliable, malware-shaped |
| B. build.rs registers | ✔ `targone` crate: adoption interface + activity beacon |
| C. cargo subcommand | ✔ `cargo-targone`: the GC engine |
| D. OS scheduler | ✔ how the engine runs unattended |
| E. cargo shim / RUSTC_WRAPPER | ◐ opt-in extra, post-1.0 |
| F. central `build.build-dir` | ✔ opt-in migration, biggest single win |
| G. shared CARGO_TARGET_DIR | ✖ rejected — correctness bugs, lock contention |
