# 07 — Recommended architecture

## Resolving the central tension

The product requirement is *"a crate added to each project that makes the
problem manage itself."* The research verdict (04) is that the thing added
to each project **must not be the thing that deletes** — a deleting
build-script is unsafe, unreliable, and malware-shaped. The resolution is a
two-part system where the per-project crate remains the entire user-facing
surface:

```
per project (crates.io)                 per machine (installed once)
┌──────────────────────────┐            ┌────────────────────────────────┐
│ targone  (dependency)    │            │ cargo-targone  (subcommand)    │
│  build.rs, ~1ms:         │  registry  │                                │
│   • register project ────┼──────────► │  ~/.targone/registry.jsonl     │
│   • stamp last activity  │            │  ~/.targone/config.toml        │
│  never deletes anything  │            │        │                       │
│  reads targone.toml      │            │        ▼ scheduled (Task       │
└──────────────────────────┘            │  GC engine  Scheduler/systemd/ │
                                        │        │      launchd timer)   │
     first build after adding           │        ▼                       │
     `targone` prints a one-line        │  per profile dir:              │
     hint if cargo-targone is           │   try_lock .cargo-build-lock   │
     not installed yet                  │   tiers P1→P6 under budget     │
                                        └────────────────────────────────┘
```

## Component 1: `targone` (the per-project crate)

- **Zero runtime code.** No library API required by users; `[build-dependencies]`
  or plain `[dependencies]` with a build.rs — decide in spec phase which
  reads better (`build-dependencies` is more honest).
- build.rs behavior (all fail-silent, total budget ~1ms):
  1. append `{schema_version, manifest_dir, workspace_root, profile,
     toolchain, timestamp}` to `~/.targone/registry.jsonl`
     (platform paths via `directories`); dedup/compaction is the engine's job;
  2. no-op under `DOCS_RS`, `TARGONE_DISABLE=1`, read-only home, CI detection
     opt-out;
  3. if `cargo-targone` is absent, emit a single
     `cargo:warning=targone: engine not installed — run: cargo install cargo-targone`
     at most once per day (stamp file).
- Optionally reads/forwards a per-project `targone.toml` so per-project
  policy lives next to the code.
- **Never touches `target/`. Never spawns processes. Never blocks.**
  This must be auditable in five minutes — it is the trust surface.

## Component 2: `cargo-targone` (the engine)

Single binary, cargo subcommand. Commands (draft):

| Command | Does |
|---|---|
| `cargo targone setup` | create OS scheduled task (Task Scheduler / systemd user timer / launchd); write machine config; optional `--central-build-dir` migration (04-F) |
| `cargo targone gc` | one GC pass: tiers P1→P6 under the budget (06); `--dry-run`, `--aggressive`, `--project <path>` |
| `cargo targone status` | per-project sizes, idleness, what the next pass would reclaim |
| `cargo targone scan <roots>` | find unregistered projects/orphaned target dirs, add to registry |
| `cargo targone mark` | P3 mark & sweep for the current project (build-driven precision tier) |
| `cargo targone uninstall` | remove task + registry |

Engine invariants (from 05):

1. Per profile dir: `try_lock` `.cargo-build-lock` exclusive + `.cargo-lock`
   shared; **on failure, skip — never wait, never proceed unlocked**; NFS →
   refuse.
2. Deletion order: fingerprint dir first, artifacts second (P2); never
   fingerprints in P3.
3. `CACHEDIR.TAG`/structural validation before any destructive act; symlink
   refusal; journal + `--dry-run`.
4. Windows: FILE_SHARE_DELETE + POSIX-semantics delete, rename-then-delete
   fallback, retry-with-backoff, tolerate residue.
5. Dual layout (legacy + build-dir v2) behind one `TargetLayout` probe.

## What ships when (proposed phases)

- **Phase 1 — engine core, vertical problem** (highest value, no build.rs
  controversy): `cargo-targone` with locking, dual-layout probe, P1
  (incremental) + P2 (toolchain sweep) + P5-orphans, `status`, `scan`,
  `gc --dry-run` default-on for the first release. Windows + Linux CI.
- **Phase 2 — set-and-forget**: `setup` (schedulers), P4 size budget, P5
  idleness tiers, machine config.
- **Phase 3 — the crates.io module**: `targone` registration crate +
  registry protocol + docs/audit story. (Deliberately after the engine has
  standalone value — the crate without the engine is a no-op.)
- **Phase 4 — precision & horizontal**: P3 mark & sweep, P6,
  `--central-build-dir` migration, `-Zno-embed-metadata`/profile hints
  surfaced in `status`.
- **Post-1.0 exploration**: cargo-shim trigger (04-E), dedup (06 out-of-
  scope list), Cargo #13136 interop as upstream lands.

## Risks & mitigations

| Risk | Mitigation |
|---|---|
| Layout v2 lands (~Oct 2026) and shifts structure again | dual-layout probe isolated in one module; CI on nightly; v2 is *easier* (per-unit dirs) |
| Fingerprint format/hash changes (happened at 1.85) | version-probe `.rustc_info.json` + fingerprint schema; fail-open like cargo-sweep; P1/P5 tiers are format-independent |
| A deletion breaks a build anyway | worst case is a rebuild by design (order rule); shakedown mode in CI; journal for forensics |
| build.rs crate perceived as spyware | local-only file, documented schema, 5-minute-auditable source, `TARGONE_DISABLE`, no network — state it in the README explicitly |
| Upstream ships #13136 / cross-workspace cache | ours is compatible-by-design (same registry model); intra-target tiers and Windows polish remain differentiated |

## Name

`targone` = **target, gone**. Engine: `cargo-targone`. Registry dir:
`~/.targone/`.
