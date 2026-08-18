# targone — the beacon crate

**`target/`, gone.** Add this crate and your project registers itself with the
[Targone engine](https://crates.io/crates/cargo-targone), which garbage-collects
superseded build artifacts across your whole machine on a schedule — under
Cargo's own file locks, keeping warm builds warm.

```toml
[build-dependencies]
targone = "0.1"
```

Then, once per machine:

```bash
cargo install cargo-targone
cargo targone schedule install   # daily, only while idle — set & forget
```

## Trust contract

This crate is compiled into your build graph, so it is deliberately tiny and
auditable in five minutes (`build.rs` + one included module, **zero
dependencies**). Its build script's single side effect is appending one JSON
line to the local file `$CARGO_HOME/targone/registry.jsonl`:

```json
{"v":1,"root":"<your project dir>","ts":<unix seconds>}
```

plus at most one `cargo:warning` per day when the engine is not installed.

It **never** deletes anything, **never** spawns a process, **never** touches
the network, emits no `rerun-if-*` directives (so it runs about once per
target dir, not per build — measured warm-build cost: **+3 ms**), and is a
hard no-op under `DOCS_RS`, `TARGONE_DISABLE=1`, or `CI`. Every failure path
is silent: this script can never break or slow your build.

All deletion lives in the engine, runs outside any build, takes Cargo's real
`.cargo-build-lock` first, and writes an append-only audit log. See the
[project README](https://github.com/hivellm/targone) for the full design and
the measured results (−89% on a 172 GB target dir, zero cold rebuilds).

## License

Apache-2.0
