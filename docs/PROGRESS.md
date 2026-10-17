# Progress

## Status

Milestone 1, plan `docs/superpowers/plans/2026-10-17-berth-m1.md`: Tasks 1-10 done.

## Done

- 2026-10-17 · Task 1 · scaffold, CLI skeleton (`up|down|ls|env|doctor`), report JSON/table, exit codes, CI · crates pinned: clap 4.6.7, serde 1.0.229, serde_json 1.0.154, toml 1.1.8, thiserror 2.0.21, tempfile 3.27.0, assert_cmd 2.2.2, predicates 3.1.4, insta 1.49.0; crates.io `berth` taken, package `keel-berth`, checked 2026-10-17
- 2026-10-17 · Task 2 · `berth.toml` loading and validation (`src/config.rs`) · `cargo test --test config` 11/11
- 2026-10-17 · Task 3 · per-worktree environment and output formats (`src/env.rs`, `PortBlock::new`) · `cargo test --test env` 8/8, sh output round-trips through `sh`
- 2026-10-17 · Task 4 · manifest I/O, port registry, atomic writes, sys helpers (`src/manifest.rs`, `registry.rs`, `sys.rs`) · `cargo test --test state` 10/10
- 2026-10-17 · Task 5 · repo and user locks with holder record, read-only `is_held` probe (`src/lock.rs`) · `cargo test --test lock` 5/5
- 2026-10-17 · Task 6 · deterministic port block allocation, bind+connect prober (`src/ports.rs`) · `cargo test --test ports` 12/12
- 2026-10-17 · Task 7 · git CLI wrapper, version check, `run` discovers the repo first (`src/git.rs`); ports prober flake fixed · `cargo test --test git` 16/16, full suite 3 runs green
- 2026-10-17 · Task 8 · copy allow-list: gitignored-only, symlink/size/tracked refusals, temp+rename into worktree (`src/copy.rs`); spec §4, §15 amended · `cargo test --test copy` 10/10
- 2026-10-17 · Task 9 · setup/teardown hook runner: own process group, group kill on timeout/failure, bounded reader grace, non-UTF-8 safe (`src/hooks.rs`); spec §8 amended · `cargo test --test hooks` 8/8
- 2026-10-17 · Task 10 · `berth up`: input checks before any lock, drift/refusal paths, ports with stale reclaim (`registry::is_stale`, read-only), info/exclude lines, `creating` entry before `worktree add`, copy, setup, rollback or `--keep-on-failure` (`src/ops/up.rs`); spec §3, §8, §9 amended · `cargo test --test cli_up` 28/28, 3 runs green
- 2026-10-17 · Task 11 · `berth down`: refusals (live creator, dirty, unmerged `--delete-branch`), teardown, only registered worktrees removed, orphan blocks freed (`src/ops/down.rs`); `up` rollback removes the root dir only if berth made it, `--keep-on-failure` copy refusal records setup `skipped`; spec §11 amended · `cargo test --test cli_down` 10/10, `cli_up` 33/33
- 2026-10-17 · Task 12 · `berth ls` (lock-free; ahead/behind, dirty, listening ports, missing/interrupted/unmanaged drift) and `berth env` (task or cwd; `--json` vs `--format`); `down` re-checks merge state before `branch -D`, says "pass --force" when worktree remove fails, reuses up's `unreserve`; spec §9/§11 amended · `cargo test --test cli_ls_env` 12/12, `cli_down` 13/13

## Blocked

Nothing yet.
