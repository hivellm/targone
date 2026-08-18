# Spike 0.3 — .fingerprint liveness

> Measured 2026-08-18 on the primary dev machine (Windows 10 Pro 19045, NTFS,
> MSVC toolchain), read-only scan of `Cortex\target\debug`,
> `Thunder\rust\target\debug`, `ar-v3-dashboard\target\debug`, plus
> `Thunder\rust\target\release`. `Cortex\target\release` exists but is an empty
> scaffold (0 fingerprint dirs); `ar-v3-dashboard` has no release profile.

## Verdict

**(a)** Only two unit types ever populate F\A (fingerprint hash absent from every artifact filename): **non-test `bin` and `example` units** — 24 of 8,539 fingerprint dirs (0.28%) across four profiles. `run-build-script` never does: 288/288 of its hashes match a `build/<pkg>-<hash>` directory name — the cargo-mark-sweep hypothesis is refuted for that type. *(high)*
**(b)** **No.** "Hash absent from artifacts" is not a safe deletion predicate for any type it actually selects: 12 of the 24 hash-absent dirs — including **7 of 7 on Cortex** — are the *live* fingerprints of the workspace's current binaries (MSVC executables get hash-less filenames, so their fingerprint hash can never appear in an artifact name). Deleting them forces a rebuild + relink of every final bin and example on the next build. *(high)*
**(c)** Safe predicate: **pairwise + orphan-recency.** (1) A fingerprint dir whose hash appears in artifact names is deleted only together with those artifacts (99.7% of dirs). (2) Among hash-absent dirs, group by (package, set of unit files) and **keep the newest `invoked.timestamp`**; older ones are stale. Verified on all 24: retains all 12 live, frees all 12 stale. Recency must be computed *within the orphan class only* — plain "newest per unit name" is unsafe (a live build-mode fingerprint can be older than a check-mode sibling of the same unit). *(high on this data; medium as a general rule)*

## Evidence

### F-S3.1 — The prior "198 excess" reconciles exactly: 191 + 7

The earlier measurement compared 3,569 Cortex fingerprint dirs against 3,371
distinct hashes **from `deps/` filenames only**. Adding `build/` top-level
directory names as artifact sources yields A = 3,371 + 191 = 3,562 distinct
hashes; F\A drops from 198 to **7**. The 191 are the build-script units (93
compile + 98 run), whose hashes appear *only* as `build/<pkg>-<hash>` directory
names, never in `deps/`. The remaining 7 are MSVC executables (F-S3.3).
191 + 7 = 198, no residue. **Confidence: high.**

### F-S3.2 — Per-profile count-by-unit-type: F vs F\A

Unit type read from the `<type>-<target>.json` filenames inside each
fingerprint dir. "absent" = hash appears in no artifact filename.

| unit type | Cortex dbg | Thunder dbg | dashboard dbg | Thunder rel |
|---|---:|---:|---:|---:|
| lib | 1,072 / 0 | 1,155 / 0 | 1,306 / 0 | 360 / 0 |
| test | 2,112 / 0 | 638 / 0 | 1,040 / 0 | — |
| bin | 193 / **7** | 50 / **6** | 36 / **4** | 1 / **1** |
| example | — | 50 / **6** | 4 / 0 | — |
| build-script (compile) | 93 / 0 | 55 / 0 | 52 / 0 | 28 / 0 |
| run-build-script | 98 / 0 | 57 / 0 | 105 / 0 | 28 / 0 |
| no-json residue | 1 / 0 | 5 / 0 | — | — |
| **total** | 3,569 / 7 | 2,010 / 12 | 2,543 / 4 | 417 / 1 |

A\F = **0** in every profile: no 16-hex artifact hash lacks a fingerprint dir.
"no-json residue" = dirs holding only `invoked.timestamp` and/or a cached
`output-*` diagnostics file, no `.json` — failed-compile leftovers, inert.
**Confidence: high** (direct enumeration).

### F-S3.3 — Why bins/examples: MSVC executables get hash-less artifact names

Every hashed `.exe` in `deps/` belongs to a **test** unit. Checked all 12
hashed exes across `cortex-api` and `cortex-workers`: fingerprint dirs with
those hashes contain `test-bin-*.json` / `test-lib-*.json`, never plain
`bin-*.json`. Plain (non-test) bin builds produce **hash-less** artifacts —
`deps\cortex_api.exe/.d/.pdb` and the uplifted `cortex-api.exe` at the profile
root — so their fingerprint hash structurally cannot appear in any artifact
filename. Examples behave identically (`examples\hello.exe`, no hash).
Multi-bin packages additionally share **one** fingerprint dir for all their
bins per build config: `cortex-workers-24a48eb2bea651a7` holds the `.json`,
`dep-`, and `output-` files of all 9 worker bins. **Confidence: high** for the
observed layout; **medium** for the mechanism attribution (matches cargo's
documented behavior of omitting the filename hash for MSVC executables —
`should_use_metadata` — but this spike did not verify cargo source).

Check-mode (`cargo check`/clippy) bin and example units are *not* orphans: they
leave a hashed `.d` dep-info file in `deps/` (examples also `lib<name>-<hash>.rmeta`
under `examples/`), e.g. fingerprint `cortex-workers-12850e3c00ace5de`
[`bin-cortex-classifier-worker.json`] ↔ `deps\cortex_classifier_worker-12850e3c00ace5de.d`.
Consequence for a mark-sweep design: `.d` files and the `examples/` directory
must count as artifact sources, or every checked bin becomes a false orphan.

### F-S3.4 — Liveness of the 24 hash-absent dirs: 12 live, 12 stale

Liveness established by `invoked.timestamp` vs the hash-less artifact's `.d`
mtime (they align within 1–5 s, same clock — all local time):

| profile | hash-absent dir | unit(s) | invoked | artifact `.d` mtime | live? |
|---|---|---|---|---|---|
| Cortex dbg | `cortex-api-80bccacf8488810e` | bin cortex-api | 08-09 23:09:58 | `cortex_api.d` 23:10:03 | **yes** |
| Cortex dbg | `cortex-workers-24a48eb2bea651a7` | 9 worker bins | 08-09 23:10:22 | `cortex_classifier_worker.d` 23:10:23 | **yes** |
| Cortex dbg | `cortex-cli-51565b6d0e741050` | 3 CLI bins | 08-09 23:09:53 | (same build run) | **yes** |
| Cortex dbg | `cortex-adapter-claude-code-566992ca…` | 2 bins | 08-09 23:09:54 | (same build run) | **yes** |
| Cortex dbg | `cortex-mcp-server-1bac4057e0ce7e4d` | bin | 08-09 23:10:03 | `cortex_mcp_server.d` 23:10:09 | **yes** |
| Cortex dbg | `cortex-core-1a2a10a6b1144ecf` | bin | 08-05 10:37:20 | `cortex_core.d` 10:37:21 | **yes** |
| Cortex dbg | `cortex-eval-7d5c35129ea3bc79` | bin | 08-05 10:37:13 | `cortex_eval.d` 10:37:13 | **yes** |
| Thunder dbg | `thunder-bench-95244e242b42a0f0` | bin | 07-19 10:48:00 | `thunder_bench.d` 10:48:01 | **yes** |
| Thunder dbg | `thunder-bench-{0f85,16be,4fe6,66fb,c6e4}…` (5) | bin | 07-18…07-19 | overwritten | no |
| Thunder dbg | `thunder-rpc-2d53ed293bc2e425` | 2 examples | 07-19 10:47:41 | `examples\hello.d` 10:47:41 | **yes** |
| Thunder dbg | `thunder-rpc-{59b5,978e,a3d2,b6f9,e284}…` (5) | 2 examples | 07-18…07-19 | overwritten | no |
| dashboard | `ar-dashboard-a87f7bc33e7d339b` | bin | 08-18 03:20:21 | `ar_dashboard.d` 03:20:22 | **yes** |
| dashboard | `ar-dashboard-e655c017c16d5d66` | bin | 08-17 02:59 | overwritten | no |
| dashboard | `ar-consolidator-36a411901e5805f2` | bin | 08-18 01:15:02 | `ar_consolidator.d` 01:15:03 | **yes** |
| dashboard | `ar-consolidator-2d850dc10052cb33` | bin | 08-17 03:36 | overwritten | no |
| Thunder rel | `thunder-bench-4fe6ee420a837fd3` | bin | 07-18 04:19:34 | `thunder_bench.d` 04:19:35 | **yes** |

Note the dashboard case: both live entries were **not** the newest fingerprint
for their unit name — newer check-mode fingerprints of the same bins exist (and
those match artifacts via `.d`). Recency is only meaningful *within* the
hash-absent group. **Confidence: high.**

### F-S3.5 — run-build-script cross-check: hypothesis refuted

Zero run-build-script entries appear in F\A, so the planned sibling-match test
is vacuous; the stronger statement holds directly: **288/288** run-build-script
fingerprint hashes (98 + 57 + 105 + 28) appear verbatim as top-level
`build/<pkg>-<hash>` directory names (the OUT_DIR side: dirs containing
`out`/`output`/`root-output`), and build-script *compile* fingerprints likewise
match the dirs containing `build_script_build.exe`. `build/` composition —
Cortex: 98 run-out + 91 compiled-script + 2 other of 191; Thunder dbg:
57 + 54 + 1 of 112; dashboard: 105 + 50 + 2 of 157; Thunder rel: 28 + 28 of 56.
**Confidence: high** on these four profiles.

### F-S3.6 — Stakes are asymmetric: orphans are bytes-free, but rebuild-expensive

Cortex `.fingerprint/` is 10.3 MB in 14,696 files for 3,569 dirs (~3 KB/dir).
Deleting the 24 hash-absent dirs recovers effectively nothing; wrongly deleting
the 12 live ones forces recompile + relink of every workspace binary (for
Cortex: 17 bins). The value of the pairing rule is not the fingerprint bytes —
it is that fingerprints must be deleted **with** their artifacts so cargo's
next build neither rebuilds live crates nor trusts a fingerprint whose
artifact is gone. A\F = 0 shows cargo itself never leaves an artifact without
its fingerprint; a sweeper should preserve that invariant in both directions.
**Confidence: high.**

## Method & caveats

Method (PowerShell, names + mtimes only; script:
`scan-fingerprint-liveness.ps1`, session scratchpad):

- **F** = every dir `<name>-<hash>` under `.fingerprint/` whose trailing
  segment after the last `-` is exactly 16 hex chars (all 8,539 dirs parsed;
  0 rejects). Recorded: file names inside, unit type from `*.json` names,
  `invoked.timestamp` mtime.
- **A** = 16-hex suffixes of: profile-root filenames, `deps/` top-level
  filenames, `build/` top-level entries (directory names included), and
  `examples/` filenames (Thunder's examples turned out to hold hashed `.d` and
  `.rmeta` files — omitting it would have inflated F\A by 44). Suffix rule:
  filename up to first `.`, rsplit on `-`, last segment iff 16 hex chars.
- Liveness: `invoked.timestamp` mtime vs the hash-less artifact family's `.d`
  mtime (1–5 s alignment observed on every live pair); unit-type of hashed
  exes confirmed by opening the owning fingerprint dir's `.json` *names*, and
  check-vs-build mode confirmed by diffing two fingerprint JSON *contents*
  (same `target`/`path` hashes, different `profile` hash).

Caveats:

- Read-only scan; file contents under `deps/`, `build/`, `incremental/` were
  never opened (atime signal preserved). Fingerprint JSON contents were read
  in two dirs only.
- All four profiles are MSVC/Windows. On non-MSVC targets plain bins get
  hashed filenames, so F\A may be empty there — the pairing rule still holds;
  the orphan-recency clause simply never fires. **Unverified here.**
- `deps/` also holds empty `rmetaXXXXXX` temp dirs (pipelining leftovers) and
  fully hash-less files (`cortex_classifier_worker.exe`); neither contributes
  to A. Uplifted root binaries are hash-less too.
- The mechanism attribution (cargo omitting `-C extra-filename` for MSVC
  executables, and the shared per-package fingerprint dir when unit metadata
  is absent) is inferred from the observed layout and matches cargo's known
  `should_use_metadata` behavior, but cargo source was not audited in this
  spike — flagged medium; the empirical tables do not depend on it.
- Doc, bench, and dylib/cdylib units did not occur in these projects'
  fingerprints; their F\A behavior is **unverified** (dylibs are also
  hash-less on Windows and plausibly behave like bins).
