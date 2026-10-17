# berth

berth gives each parallel coding agent an isolated, ready workspace in a git repo: `berth up
<task>` creates a worktree, copies local env files, reserves a block of ports and runs your setup
hook, and `berth down <task>` cleans up. One static Rust binary, part of the keelstackhq family
(hull, splice, hoard, hoist). Status: M1 (local use, built from source). Design:
`docs/superpowers/specs/2026-10-17-berth-design.md`.

## Install

From source (Rust 1.97, git >= 2.36):

```bash
cargo install --path .        # package `keel-berth`, installs the binary `berth`
# or: cargo build --release   # target/release/berth
```

## Commands

```
berth up <task> [--base REF] [--branch NAME] [--no-setup] [--keep-on-failure] [--json]
berth down <task> [--force] [--delete-branch] [--json]
berth ls [--json]
berth env [<task>] [--format sh|fish|dotenv|json]
berth doctor [--json]
```

Global flags: `-C <dir>`, `--json`, `--no-color`, `-v` (prints each git command and hook command
to stderr, never file contents or env values). `<task>` is a slug `^[a-z0-9][a-z0-9-]{0,47}$`.

- `up` makes the worktree and branch (`berth/<task>`), copies the allow-listed untracked files,
  reserves a port block and runs `[setup]`. Repeating it on a healthy task is `ok`; a different
  `--base`, `--branch` or port count is `drift`.
- `down` runs `[teardown]` and removes the worktree and its port block. It refuses a dirty
  worktree (`--force` overrides). The branch stays unless `--delete-branch` and it is merged
  into `[worktree].base`.
- `ls` lists worktrees with state, ahead/behind, dirty and listening ports. `env` prints the
  environment of a task, or of the worktree you are in: `eval "$(berth env)"`.
- `doctor` checks git, config, copy rules, root, port registry, manifest and owners.
  `ls`, `env` and `doctor` take no locks and write nothing.

Worktrees default to a sibling directory `../<repo>.berth/<task>`.

## berth.toml

Optional seed file at the repo root; every key is optional, unknown keys are an error.

```toml
version = 1
[worktree]  # root = "../{repo}.berth", branch_prefix = "berth/", base = "HEAD", max = 6
[copy]      # include = [".env", ...], max_bytes = 1048576; files must be gitignored
[ports]     # count = 10, range = [21000, 30999], names = { web = 0, api = 1, ... } -> WEB_PORT, ...
[env]       # non-secret values; {repo} {task} {task_snake} {port.<name>} {port.base} {worktree}
[setup]     # run = ["pnpm install --frozen-lockfile"], timeout_secs = 900
[teardown]  # same keys as [setup]; runs before removal
```

`berth env` also sets `BERTH_TASK`, `BERTH_ROOT`, `BERTH_WORKTREE`, `BERTH_PORT_BASE`,
`BERTH_PORT_COUNT`, `PORT`, `TURBO_CACHE_DIR`, `VITE_CACHE_PATH`, `CARGO_TARGET_DIR` and `TMPDIR`
(all caches inside the worktree). Copied files' values are never exported.

## Exit codes

`0` ok · `1` refused or drift · `2` usage · `3` config or environment error (the message says how
to fix it). With `--json`, stdout is exactly one object
`{version, command, ok, exit_code, items, worktrees}`; logs and hook output go to stderr.

## Environment variables

- `BERTH_STATE_DIR`: state directory (port registry, user lock). Default `$XDG_STATE_HOME/berth`,
  else `~/.local/state/berth`.
- `BERTH_LOCK_TIMEOUT_MS`: how long to wait for a lock before exit 3 (default 30000).

## Notes

- Hooks run in their own process group, so Ctrl-C does not reach them. A killed `berth` leaves
  the task as `drift: interrupted`; run `berth down <task>` and `berth up` again.
- A task whose owner runs on another host counts as alive: `ls` never clears it. `doctor`
  reports a `creating` entry owned by another host as drift ("cannot verify").
- berth never edits tracked files. The lines it adds to `.git/info/exclude` stay after
  `down`; they are harmless.
- A repo whose `.gitignore` lacks `.turbo/` or `target/` will see `down` refuse after builds
  (the worktree is dirty). Add them to `.gitignore` or use `--force`.
