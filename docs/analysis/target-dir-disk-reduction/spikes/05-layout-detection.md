# Spike 0.5 — layout detection

> Executed 2026-08-18 with three sandbox builds on the reference machine:
> stable cargo 1.97.1 (legacy unified layout), stable 1.97.1 with
> `build.build-dir` (split layout), and cargo 1.99.0-nightly 2026-07-17 with
> `-Zbuild-dir-new-layout` (layout v2). Full tree listings captured; sandboxes
> in the session scratchpad (`spike01-sandbox`, `spike05-split`, `spike05-v2`).

## Verdict

**Resolution mechanism (settled):** `cargo metadata --no-deps` exposes BOTH
`target_directory` and `build_directory` on stable 1.97 — the engine resolves
the two roots per workspace from metadata, never by guessing.
**Critical trap, observed directly:** cargo config discovery is
**CWD-based, not manifest-path-based**. A build (and metadata query) invoked
from outside the project with `--manifest-path` silently IGNORED the
project's `.cargo/config.toml` `build-dir` and used the unified layout.
The engine must run `cargo metadata` with CWD = the workspace directory.

**Structural detection grammar** (config-independent, probe the profile dir):

| Observation | Layout | Sweepable pools present |
|---|---|---|
| `.fingerprint/` + `deps/` present | **legacy build side** (unified or the build-dir half of a split) | `.fingerprint/`, `deps/`, `build/<pkg>-<16hex>/`, `incremental/` |
| `build/<pkg>/<16hex>/fingerprint/` present; no `.fingerprint/`, no `deps/` | **layout v2** | per-unit dirs `build/<pkg>/<META>/{fingerprint,out[,run]}`, `incremental/` |
| `.cargo-artifact-lock` + uplifted artifacts + `examples/`; none of the above | **artifact-only side of a split** | nothing intra-profile — sweep happens in its build-dir counterpart |
| none of the above | **unknown → fail closed: sweep nothing, report** | — |

**Fail-closed rule:** classification requires a positive grammar match; an
unmatched profile dir is reported and skipped, never "best-effort" swept.

Confidence: high — every row above is a captured directory listing, not an
inference.

## Evidence

### Legacy unified (stable 1.97.1 default)

```
target/                          target/debug/
  .rustc_info.json                 .fingerprint/  build/  deps/
  CACHEDIR.TAG          ←(root!)   examples/  incremental/
  debug/                           .cargo-lock  .cargo-build-lock  .cargo-artifact-lock
                                   <uplifted exe/.d/.pdb>
```

Note: `CACHEDIR.TAG` observed at the **target root** on 1.97.1 (the
2020-era PR wrote it into profile dirs; current cargo writes the root).
Detection must accept it at either level — and F-055 already showed its
absence is possible, so it stays a *positive* signal only, never a required
one.

### Split (`build.build-dir`, stable since 1.91)

```
artifact-dir (target/)           build-dir (configured path)
  CACHEDIR.TAG                     .rustc_info.json
  debug/                           CACHEDIR.TAG
    examples/                      debug/
    .cargo-lock                      .fingerprint/  build/  deps/
    .cargo-artifact-lock             examples/  incremental/
    <uplifted exe/.d/.pdb>           .cargo-build-lock
```

Consequences: `.rustc_info.json` moves to the build-dir root (any detector
keying on it under `target/` breaks — cargo-wipe's known fragility,
confirmed); the sweep lock (`.cargo-build-lock`) lives ONLY in the build-dir
profile; the artifact-dir profile keeps `.cargo-lock` + `.cargo-artifact-lock`.

### Layout v2 (`-Zbuild-dir-new-layout`, cargo 1.99.0-nightly 2026-07-17)

```
target/debug/
  build/<pkgname>/<16hex-META>/
    fingerprint/    ← same file grammar as legacy .fingerprint/<pkg>-<hash>/
                      (dep-lib-*, invoked.timestamp, lib-*, lib-*.json / bin-*)
    out/            ← the unit's artifacts (libserde-<hash>.rlib/.rmeta/.d,
                      or exe/.pdb for bins)
    run/            ← present only for build-script-run units
  examples/  incremental/           ← unchanged, still profile-flat
  .cargo-lock  .cargo-build-lock  .cargo-artifact-lock
  <uplifted exe/.d/.pdb>
```

No `.fingerprint/`, no `deps/`. The unit hash is still 16 lowercase hex.
Package names keep their hyphens at the `build/<pkgname>/` level (e.g.
`spike05-v2/`, `unicode-ident/`) — no underscore sanitization there.
Deletion in v2 is per-unit-directory: atomic, no cross-directory ordering —
strictly easier than legacy. `incremental/` handling is identical across all
three layouts.

## Minimum assumption set for the engine

1. Lock files are named `.cargo-lock` / `.cargo-build-lock` /
   `.cargo-artifact-lock` and live in profile dirs (all three layouts —
   verified).
2. Unit hashes are 16 lowercase hex, joined across fingerprint↔artifacts
   (legacy) or embodied as the `<META>` dir name (v2).
3. `cargo metadata --no-deps`, run with CWD inside the workspace, is the
   sole authority for the target/build roots.
4. Everything else (marker file positions, pool names) is layout-specific
   and must come from the grammar table above.

## Method & caveats

- Sandboxes are single-crate projects with serde/serde_json; workspace and
  `--target` cross-compile trees were not exercised (their profile dirs
  nest under `<triple>/` but carry the same grammar — verify in phase 1
  tests).
- v2 was built by passing `-Zbuild-dir-new-layout` explicitly; whether the
  tested nightly enables it by default was not probed. Stabilization is
  tracked for ~1.99 (see 09-cargo-upstream-roadmap.md).
- `run/` directory internals in v2 were not inventoried (not needed for
  the detection grammar; needed later for OUT_DIR-aware tiers).
