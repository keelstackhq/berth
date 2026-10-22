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
berth env [<task>] [--format sh|fish|pwsh|dotenv|json]
berth doctor [--json]
berth gc [--dry-run] [--delete-branch] [--json]
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
  environment of a task, or of the worktree you are in: `eval "$(berth env)"` (PowerShell: `berth env --format pwsh | Out-String | Invoke-Expression`).
- `doctor` checks git, config, copy rules, root, port registry, manifest and owners, plus the
  database tools, the sqlite template, turbo seeding and the hoard server when configured.
  `ls`, `env` and `doctor` take no locks and write nothing.
- `gc` runs `down` for every `ready` task whose worktree is clean and whose branch has commits
  that are all merged into `[worktree].base` (fast-forward, merge, or, with git 2.38+, squash or
  rebase). Fresh, dirty and unmerged tasks are kept. berth never fetches: `git pull` first.
  `--dry-run` only reports.

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
[db]        # provider = "sqlite" | "postgres" | "compose"; see below
[cache]     # turbo_seed = "off" | "link" | "copy", remote = "none" | "hoard", hoard_dir = ".hoard"
```

### Databases (`[db]`)

Each task gets its own database; the URL goes to `[db].env` (default `DATABASE_URL`, `""` for
none). `up` creates it after copying files and before `[setup]`, so setup can run migrations;
`down` (and `gc`) drop it after `[teardown]`.

```toml
[db]                        # sqlite: copy a template file into the worktree
provider = "sqlite"
template = "db/dev.sqlite"  # in the main checkout; path = "..." to put it elsewhere (must be gitignored)

[db]                        # postgres: createdb --template on a local server
provider = "postgres"
template = "app_dev"        # optional; name = "<repo>_<h>_<task_snake>" by default (max 63, [a-z0-9_]; <h> = 6 hex chars of the checkout path)
host = "localhost"          # port = 5432, user = "..." (optional)

[db]                        # compose: one compose project per task
provider = "compose"
services = ["db"]           # docker compose up --detach --wait; project = "<repo>-<h>-<task>" by default (<h> = 6 hex chars of the checkout path)
url = "postgres://postgres@localhost:{port.db}/app"   # optional
```

- berth never sees a password: `createdb`/`dropdb` run with `--no-password` and read
  `PGPASSWORD` or `~/.pgpass` themselves. A template with open sessions makes `createdb` fail
  after 5 s; stop the main dev server first.
- berth drops only a database it created (recorded in the manifest). An existing database of the
  same name makes `up` fail. `down --force` also ends open sessions (`dropdb --force`).
- compose gets `COMPOSE_PROJECT_NAME` and the `<NAME>_PORT` variables, so a compose file maps
  ports with `${DB_PORT:-5432}:5432`. `down` runs `docker compose down --volumes`.
- sqlite: a non-empty `<template>-wal` is an error (checkpoint it or stop the app).

### Caches (`[cache]`)

- `turbo_seed = "link"` hard-links the main checkout's `.turbo/cache` entries into the new
  worktree (turbo writes entries by temp file and rename, so the main copy never changes;
  `copy` copies instead). `.turbo/` must be gitignored.
- `remote = "hoard"` points worktrees at a `hoard serve --storage fs` you start in the main
  checkout (it writes `.hoard/port` and `.hoard/token`). `berth env` exports `TURBO_API`,
  `TURBO_TEAM` and `TURBO_TOKEN="$(cat <token file>)"`, so the token itself is never printed
  (`--format dotenv|json` leave it out); hooks get the real value. Nothing about hoard is stored
  in the manifest.

`berth env` also sets `BERTH_TASK`, `BERTH_ROOT`, `BERTH_WORKTREE`, `BERTH_PORT_BASE`,
`BERTH_PORT_COUNT`, `PORT`, `TURBO_CACHE_DIR`, `VITE_CACHE_PATH`, `CARGO_TARGET_DIR` and `TMPDIR`
(all caches inside the worktree). Copied files' values are never exported.

## Exit codes

`0` ok · `1` refused or drift (also: a `[db]` sqlite target git does not ignore, `gc` when a
`down` refused) · `2` usage · `3` config or environment error (the message says how to fix it;
also: missing `createdb`/`dropdb`/`docker`, a failed database step). With `--json`, stdout is exactly one object
`{version, command, ok, exit_code, items, worktrees}`; logs and hook output go to stderr.

## Environment variables

- `BERTH_STATE_DIR`: state directory (port registry, user lock). Default `$XDG_STATE_HOME/berth`,
  else `~/.local/state/berth`.
- `BERTH_LOCK_TIMEOUT_MS`: how long to wait for a lock before exit 3 (default 30000).

## Notes

- Hooks run in their own process group, so Ctrl-C does not reach them. A killed `berth` leaves
  the task as `drift: interrupted`; run `berth down <task>` and `berth up` again.
- A task whose owner runs on another host counts as alive: `ls` never clears it. `doctor`
  reports a `creating` entry owned by another host as drift ("cannot verify"). `down` refuses
  it too; if that `up` is gone, `berth down --force <task>` removes it. A live `up` on this host
  always refuses `down`, even with `--force`.
- berth never edits tracked files. The lines it adds to `.git/info/exclude` stay after
  `down`; they are harmless.
- A repo whose `.gitignore` lacks `.turbo/` or `target/` will see `down` refuse after builds
  (the worktree is dirty). Add them to `.gitignore` or use `--force`.
