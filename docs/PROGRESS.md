# Progress

## Status

Milestone 1, plan `docs/superpowers/plans/2026-10-17-berth-m1.md`: Tasks 1-4 done.

## Done

- 2026-10-17 · Task 1 · scaffold, CLI skeleton (`up|down|ls|env|doctor`), report JSON/table, exit codes, CI · crates pinned: clap 4.6.7, serde 1.0.229, serde_json 1.0.154, toml 1.1.8, thiserror 2.0.21, tempfile 3.27.0, assert_cmd 2.2.2, predicates 3.1.4, insta 1.49.0; crates.io `berth` taken, package `keel-berth`, checked 2026-10-17
- 2026-10-17 · Task 2 · `berth.toml` loading and validation (`src/config.rs`) · `cargo test --test config` 11/11
- 2026-10-17 · Task 3 · per-worktree environment and output formats (`src/env.rs`, `PortBlock::new`) · `cargo test --test env` 8/8, sh output round-trips through `sh`
- 2026-10-17 · Task 4 · manifest I/O, port registry, atomic writes, sys helpers (`src/manifest.rs`, `registry.rs`, `sys.rs`) · `cargo test --test state` 10/10

## Blocked

Nothing yet.
