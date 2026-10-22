---
name: berth
description: Use when you start work on a task in a git repo that runs several agents or checkouts in parallel, or when you need your own ports, caches, database or env files without colliding with other worktrees. berth gives each task an isolated, ready git worktree.
---

# berth

berth (`berth up|down|ls|env|gc`) gives every parallel task its own git worktree, a reserved
block of ports, isolated caches and an optional database. Use it instead of `git worktree add`
or a second clone, and never start a dev server on a fixed port.

## When to call what

- **Start a task:** `berth up <task> --json`. `<task>` is a slug: `a-z`, `0-9`, `-`, at most 48
  characters (`fix-login`). Calling it again on a healthy task is safe and returns `ok`.
- **Get the environment** (ports, cache dirs, database URL) in the current shell:
  `eval "$(berth env)"` from inside the worktree, or `berth env <task>`. Formats:
  `--format sh|fish|dotenv|json`. Skip this when your session already exports it
  (`echo $BERTH_TASK` is set).
- **See what exists:** `berth ls` (state, ahead/behind, dirty, listening ports).
- **Finish a task:** commit your work, then `berth down <task>`. It refuses a dirty worktree and
  keeps the branch. Add `--delete-branch` only after the branch is merged.
- **Clean up after merges:** `berth gc --dry-run`, then `berth gc` removes merged, clean,
  ready tasks.
- **Something looks off:** `berth doctor`.

## Reading `berth up <task> --json`

stdout is exactly one JSON object:
`{ "version": 1, "command", "ok", "exit_code", "items": [...], "worktrees": [ ... ] }`.

- `worktrees[0].path`: absolute path of the worktree. Work (and run tools) there.
- `worktrees[0].branch`: the task branch (`berth/<task>` by default).
- `worktrees[0].ports`: `{ "start", "end", "named": { "web": 21170, ... } }`.
- `worktrees[0].env`: the variables to use. Always present: `PORT` (first port of your block),
  `<NAME>_PORT` for each named port (`WEB_PORT`, `API_PORT`, ...), `BERTH_TASK`,
  `BERTH_WORKTREE`, `BERTH_ROOT`, `TURBO_CACHE_DIR`, `CARGO_TARGET_DIR`, `TMPDIR`; and
  `DATABASE_URL` when the repo configures a database.
- `items`: one entry per step, each `{ kind, key, action, reason }` with action `create`, `ok`,
  `remove`, `drift`, `refused` or `skipped`. Read `reason` of any `refused` or `drift` item.

## Exit codes

- `0` done.
- `1` refused or drift: nothing was changed. Read `items` for the reason (worktree limit
  reached, task exists with different settings, dirty worktree on `down`, a copy-listed file
  that git does not ignore). Fix the cause or choose another task name; do not retry blindly.
- `2` usage error: bad task slug or flags. Fix the command.
- `3` configuration or environment error (bad `berth.toml`, git too old, no free port block,
  setup hook failed, lock timeout). The message on stderr says how to fix it. Do not work
  around it by creating the worktree by hand.

## Rules

- **Never read, print, copy or edit the files berth copied** (`[copy]` in `berth.toml` or
  `.worktreeinclude`, typically `.env`, `.env.local`, `.dev.vars`). They hold secrets. Use the
  environment variables instead. `worktrees[0].copied` lists the paths, never their content.
- **Use `$PORT` / `$<NAME>_PORT`**, never a fixed port such as 3000 or 5173. Pass them to servers
  (`vite --port "$WEB_PORT"`) and to test runners.
- **Stay inside your worktree.** Do not touch the main checkout or another task's directory.
- Do not edit `.agents/worktrees.json` or `berth.toml` to change your allocation; use the
  commands.
- Logs, git output and setup-hook output go to stderr; parse stdout only.
