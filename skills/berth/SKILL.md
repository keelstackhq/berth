---
name: berth
description: Use when you start work on a task in a git repo that runs several agents or checkouts in parallel, or when you need your own ports, caches, database or env files without colliding with other worktrees. berth gives each task an isolated, ready git worktree.
---

# berth

berth (`berth up|down|ls|env|gc`) gives every parallel task its own git worktree, a reserved
block of ports, isolated caches and an optional database. Use it instead of `git worktree add`
or a second clone, and never start a dev server on a fixed port.

## When to call what

- **Start a task:** `berth up <task> --json`. `<task>` is a slug (`fix-login`: `a-z`, `0-9`, `-`, max 48).
  Calling it again on a healthy task is safe and returns `ok`.
- **Environment:** `eval "$(berth env)"` inside the worktree; skip it when `$BERTH_TASK` is set.
- **See what exists:** `berth ls` (state, ahead/behind, dirty; ports show only while something listens).
- **Finish a task:** commit your work, then `berth down` (inside the worktree) or
  `berth down <task>`. It refuses a dirty worktree (untracked files count: `git add` and commit
  them; `--force` discards them) and keeps the branch. Add `--delete-branch` only after the
  branch is merged. Run from inside the worktree, it removes your cwd: `cd` to the main checkout
  afterwards. `down` of an unknown task exits 0 with `no such task`: check the name with `berth ls`.
- **Clean up after merges:** `berth gc --dry-run`, then `berth gc` (merged, clean, ready tasks).
  **Killed or odd state:** run `berth up <task>` again (it redoes an interrupted one), or
  `berth doctor --fix` (removes interrupted tasks and stale ports).

## Reading `berth up <task> --json`

stdout is exactly one JSON object:
`{ "version": 1, "command", "ok", "exit_code", "items": [...], "worktrees": [ ... ] }`.

- `worktrees[0].path`: absolute path of the worktree. Work (and run tools) there.
- `worktrees[0].branch` (`berth/<task>` by default) and `.ports`:
  `{ "start", "end", "named": { "web": 21170, ... } }`.
- `worktrees[0].env`: the variables to use: `PORT` (first port), `<NAME>_PORT` per named port (default names `web` `api` `db` `storybook`; `berth.toml` sets them),
  `BERTH_TASK`, `BERTH_WORKTREE`, `BERTH_ROOT`, `TURBO_CACHE_DIR`, `CARGO_TARGET_DIR`, `TMPDIR`,
  and `DATABASE_URL` when the repo configures a database.
- `items`: one entry per step, each `{ kind, key, action, reason }` with action `create`, `ok`,
  `remove`, `drift`, `refused` or `skipped`. Read `reason` of any `refused` or `drift` item.

## Exit codes

- `0` done. `2` usage error: fix the command (the message suggests a valid slug).
- `1` refused or drift, nothing changed: read `items[].reason` (worktree limit, task exists with
  other settings, dirty worktree on `down`, a copy-listed file git does not ignore). Fix the
  cause or pick another task name; do not retry blindly.
- `3` config or environment error (bad `berth.toml`, git too old, no free port block, setup
  hook failed, lock timeout). stderr says how to fix it; never create the worktree by hand.

## Rules

- **Never read, print, copy or edit the files berth copied** (`[copy]` in `berth.toml` or
  `.worktreeinclude`, typically `.env`, `.env.local`, `.dev.vars`). They hold secrets. Use the
  environment variables instead. `worktrees[0].copied` lists the paths, never their content.
- **Use `$PORT` / `$<NAME>_PORT`**, never a fixed port such as 3000 or 5173. Pass them to servers
  (`vite --port "$WEB_PORT"`) and to test runners.
- **Stay inside your worktree**; never touch the main checkout or another task's directory, and
  never edit `.agents/worktrees.json` or `berth.toml` to change your allocation.
- Errors with `--json` still print one JSON object (`ok:false`). Logs and hook output go to
  stderr; parse stdout only.
