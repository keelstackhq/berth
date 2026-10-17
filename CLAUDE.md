# berth

Give each parallel coding agent an isolated, ready workspace in a git repo: `berth up <task>`
makes a worktree, copies local env files, reserves a port block and runs setup. One static Rust
binary. Part of the keelstackhq family (hull, splice, hoard, hoist).

- Spec: `docs/superpowers/specs/2026-10-17-berth-design.md` — source of truth. Read only the
  section you touch (§3 CLI, §4 config, §6 manifest, §8 up, §9 ports); update it in the same
  commit when behaviour changes.
- Plans: `docs/superpowers/plans/` (one per milestone). Tick tasks as they land.
- Progress: `docs/PROGRESS.md` — Status, Done (one line per task), Blocked (human-only items).

## Layout

```
src/main.rs cli.rs lib.rs   clap (usage errors -> 2) -> run(Cli) -> Report -> table/JSON/text
src/error.rs report.rs      BerthError (2 usage, 3 rest); Action, Item, Report; render_*
src/manifest.rs             Manifest/Entry types (.agents/worktrees.json); registry, lock, ports,
                            git, copy, hooks and ops/{up,down,ls,env,doctor} follow per plan
tests/common/mod.rs         Fixture: hermetic repo, HOME, state dir, TMPDIR, port window
tests/cli_*.rs              integration tests; insta snapshots in tests/snapshots/
.github/workflows/ci.yml    job ci (ubuntu, macos) + ci-ok
```

## Commands

```bash
export PATH=/root/.cargo/bin:$PATH
cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked
cargo test --test cli_up          # one file
```

Run the fast checks before every commit.

## Rules that must never break

- **Exit codes:** 0 ok · 1 refused or drift · 2 usage · 3 config or environment error (the
  message says how to fix it). Refusals are `Item`s with `Report.exit_code = 1`.
- **`--json` stdout is exactly one object** `{version:1, command, ok, exit_code, items,
  worktrees}`, also for errors. Logs, git and hook output go to stderr.
- **Actions:** `create`, `ok`, `remove`, `drift`, `refused`, `skipped`.
- **Never print secrets:** copied file contents, hashes and env values stay out of stdout,
  stderr, JSON, the manifest and logs. `-v` logs command lines to stderr only.
- **Never edit tracked files.** Only append to `<git-common-dir>/info/exclude`.
- **Atomic writes:** `.<name>.tmp.<pid>`, `sync_all`, `rename`.
- **Lock order repo → user.** Never block on a repo lock while holding the user lock. `ls`,
  `env`, `doctor` take no locks.
- **git via CLI only**, version >= 2.36, user refs after `--end-of-options`. No network or TLS
  crates; `#![forbid(unsafe_code)]`.
- **Hermetic tests:** use `tests/common`; ports only from `port_range()`; no real `~`.

## How to work here

- Test first: failing test, watch it fail, implement, watch it pass.
- One plan task per commit, with its PROGRESS line; message as in the plan.
- Pin crates exactly (`=x.y.z`); ask before adding any crate not in the plan's Tech Stack.
- Read big files by section (offset/limit or grep), not whole.
- Show real command output before claiming something works.
- Snapshots: run the test, read every `.snap.new`, then `cargo insta accept` (if installed) or
  `INSTA_UPDATE=always cargo test --test <file>`. Never blind-accept.
- Human-only items (releases, npm scopes, repo settings) go to Blocked in `docs/PROGRESS.md`.

## Gotchas

- `TMPDIR` may be `/`; set it to a writable dir before `cargo test`.
- `.cargo/config.toml` points git at an empty global config for `cargo test` and `cargo run`.
- `git check-ignore` exits 1 when nothing is ignored; that is not an error.
- `info/exclude` lives in the common git dir and applies to every worktree.
- flock conflicts across file descriptors even inside one process.
- The package is `keel-berth`; the binary and the lib are `berth`.

## Org decisions

Cross-repo decisions from keelstackhq/decisions; follow them. To change one, open a PR there.
The hook refreshes this snapshot; commit an update alone as `chore: sync org decisions`.

@docs/agents/org-decisions.md
