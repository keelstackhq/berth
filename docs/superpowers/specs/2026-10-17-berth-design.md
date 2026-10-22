# berth — Design Spec

Date: 2026-10-17 · Status: approved (exit codes follow splice, §3)

Names: repo `keelstackhq/berth`, binary `berth`, Cargo package `keel-berth` (crates.io `berth`
taken, checked 2026-10-17), lib and bin `berth`, npm launcher `@keelstackhq/berth` with
platform packages `@keelstackhq/berth-{linux-x64,linux-arm64,darwin-x64,darwin-arm64}` (the splice
pattern; `win32-x64` from M3).

## 1. Goal

One headless command gives each parallel coding agent a ready, isolated workspace in a repo:
`berth up <task>` creates a git worktree on its own branch, brings over the untracked files the
repo needs (`.env` and friends) without showing them to anyone, allocates a block of free ports,
gives the worktree its own caches and database, runs the repo's setup, and records all of it in a
manifest other tools can read. `berth down <task>` undoes all of it and refuses to lose work.

Why (evidence: `reports/Developer and agent DX ecosystem.md` §berth; research notes
`agent_pain_points.md` §9, `toolchain_gaps_2.md` §6, `user_demands.md` Theme 4): agents sharing
one checkout clobber each other; fresh worktrees lack gitignored files (`.env`); parallel dev
servers and tests fight over ports and one database; shared caches "stomp on each other"
(HN 47004368).
- Incumbents are GUIs (Conductor, Vibe Kanban, Sculptor, Emdash) with OAuth or telemetry
  objections, or sandboxes (Docker `sbx`, container-use) that isolate but document no port or DB
  allocation. Nobody owns the headless layer under them. Users also say "I wrote my own", so berth
  must stay small enough to be obviously cheaper than writing it.

Success:
- Two `berth up` started at the same instant on example-app give disjoint ports, caches and DBs;
  both run `pnpm run ci` and their dev servers at the same time.
- `up` then `down` leaves repo, port registry and disk as before, except the kept task branch.
- Any harness (Claude Code, Codex, a GUI, a script) drives berth through `--json` and exit codes.

### Non-goals (v1)

- A sandbox. berth isolates *state* (files, ports, caches, DBs), not processes; egress control
  and microVMs stay with `sbx`, container-use or the harness.
- A GUI, agent launcher, daemon or hosted service. Every command does its work and exits.
- Reviewing or merging agent work (the real bottleneck). `berth ls` shows which tasks have commits
  and which are dirty; comparing diffs across agents is a later tool or a GUI's job.
- Secret management: berth copies files the owner already has; it never fetches or mints them.

## 2. Positioning

`git worktree`/`claude --worktree` give the worktree only; GUIs add a board but no documented port
or DB allocation; `sbx`/container-use isolate processes but forward ports by hand. berth adds the
missing per-task state (files, ports, caches, DB, manifest) and composes with all of them: a GUI or
`sbx` can call `berth up --json` and use the returned path, ports and env.

## 3. CLI

```
berth up <task> [--base REF] [--branch NAME] [--no-setup] [--keep-on-failure] [--json]
berth down <task> [--force] [--delete-branch] [--json]
berth ls [--json]
berth env [<task>] [--format sh|fish|pwsh|dotenv|json]   task defaults to the worktree of the cwd
berth gc [--dry-run] [--delete-branch] [--json]
berth doctor [--json]
```

Global flags: `-C <dir>`, `--json`, `--no-color`, `-v`. `<task>` is a slug
`^[a-z0-9][a-z0-9-]{0,47}$` (it becomes a path segment, a branch suffix and a DB name).

### Exit codes

`0` ok · `1` refused or drift (dirty worktree on `down`, task exists with different settings, a
copy-listed file git does not ignore, worktree limit reached, task being created by a live `up`:
"task is being created by pid N", `doctor` found drift, a `gc` removal that `down` refused) · `2` usage (bad flags or arguments,
invalid task slug, invalid `--branch`; clap usage errors exit 2) · `3` config or environment
error (unknown `berth.toml` key, git missing or too old, unknown or `-`-prefixed `--base`: "unknown
--base ref `<x>`; berth never fetches, run `git fetch` first", no free port block, lock timeout,
setup hook failed, a `gc` removal that failed); the message says how to fix it.

Justification: berth follows splice's scheme (0 ok · 1 refused/drift · 2 usage · 3 config or
environment) because berth has no human steps, which is what hoist's `2` means. Agents only need
the rule shared by the family: `0` go, `1` look at `items`, `3` fix the environment.

### Output

Human output: a short table plus the next command (`cd ../hull.berth/fix-login && eval
"$(berth env)"`; on Windows `Set-Location -LiteralPath '..\hull.berth\fix-login'; berth env
--format pwsh | Out-String | Invoke-Expression`). Logs and hook output go to stderr.

`--json` prints exactly one object on stdout:
`{ "version": 1, "command", "ok", "exit_code", "items": [ { "kind", "key", "action", "reason" } ],
"worktrees": [ <manifest entry, §6> ] }`. Actions: `create`, `ok`, `remove`, `drift`, `refused`,
`skipped`. `up` and `env` return one entry in `worktrees`; `ls` returns all. Copied file contents
never appear in either stream (§8).

### Idempotency

`berth up t` on an existing, healthy task with unchanged config prints the same result with action
`ok` and exit 0, so a harness can call it on every session start. Same task, different `--base`,
`--branch` or port count → `drift`, exit 1, nothing changed. `berth down t` on an unknown task →
`ok`, exit 0.

## 4. `berth.toml`

A seed file at the repo root (hull renders it; the repo owns it afterwards). Unknown keys are an
error (exit 3, names the key and the file). Every key is optional; no file means the defaults.

```toml
version = 1

[worktree]
root = "../{repo}.berth"        # where worktrees live (§5); {repo} = main checkout dir name
branch_prefix = "berth/"        # branch = branch_prefix + task
base = "HEAD"                   # default --base; resolved in the main checkout, never fetched
max = 6                         # refuse a 7th (laptop ceiling reported at ~5-7 agents)

[copy]                          # untracked files copied from the main checkout
include = [".env", ".env.local", "apps/*/.env.local", ".dev.vars"]
max_bytes = 1048576             # per file; larger → refused

[ports]
count = 10                      # block size
range = [21000, 30999]          # below Linux (32768+) and macOS (49152+) ephemeral ranges
names = { web = 0, api = 1, storybook = 2, db = 3 }   # → WEB_PORT=base+0, API_PORT=base+1, ...

[env]                           # non-secret values; {placeholders} from §7
# With [db] below, DATABASE_URL and (compose) COMPOSE_PROJECT_NAME come from [db] and are rejected
# here; without [db] you could set them yourself, e.g. DATABASE_URL = "postgres://localhost/app_{task_snake}"

[db]                            # optional; absent = no database handling
provider = "postgres"           # required: "sqlite" | "postgres" | "compose"
env = "DATABASE_URL"            # variable that receives the URL; "" exports none
url = "..."                     # URL template (§7 placeholders, {db.name}, {db.path}); default per provider
timeout_secs = 300              # per createdb/dropdb/docker call
template = "app_template"       # sqlite: file to copy (required); postgres: template database
path = "dev.db"                 # sqlite only: file inside the worktree (default = template)
name = "app_{task_snake}"       # postgres only: database name (default <repo>_<h>_<task_snake>, §7)
host = "localhost"              # postgres only
port = 5432                     # postgres only
user = "me"                     # postgres only; no password is ever handled
services = ["db"]               # compose only: services to start (default all)
project = "{repo}-{task}"       # compose only: project name (default <repo>-<h>-<task>, §7)

[cache]
turbo_seed = "off"              # "off" | "link" | "copy": seed .turbo/cache from the main checkout
remote = "none"                 # "none" | "hoard": export a running `hoard serve` to the worktree
hoard_dir = ".hoard"            # relative to the main checkout: holds hoard's port and token files

[setup]                         # run in the new worktree, in order, after copy; stderr streamed
run = ["pnpm install --frozen-lockfile"]
timeout_secs = 900              # [teardown] has the same keys; runs before removal
```

`[db]` and `[cache]` config errors (exit 3, naming the key): unknown `provider`; a `[db]` key for
the wrong provider (`template` sqlite/postgres, `path` sqlite, `name`/`host`/`port`/`user`
postgres, `services`/`project` compose); sqlite without `template`; sqlite `template`/`path`
empty, absolute or containing `..`; `env` not `""` or `[A-Z_][A-Z0-9_]*` or reserved; bad characters
in `host`, `user`, a postgres `template` or `services`; `timeout_secs = 0`; `[env]` setting the
`[db].env` variable (use `[db].url`), `COMPOSE_PROJECT_NAME` with compose (use `[db].project`) or
`TURBO_API`/`TURBO_TOKEN`/`TURBO_TEAM` with `remote = "hoard"`; unknown `[cache]` keys or values.

No `[copy]` but a `.worktreeinclude` file → its patterns are used (confirm format at plan time).
`.worktreeinclude` matches that git does not ignore are `skipped` (Claude Code behaviour); `[copy]`
matches that git does not ignore are `refused`.

## 5. Worktree location (decision)

Default: a **sibling directory** `../<repo>.berth/<task>`, not `.worktrees/` inside the repo.

- Node resolution walks up: a worktree under `<repo>/.worktrees/x/` resolves a missing dependency
  from the main checkout's `node_modules`, hiding phantom-dependency bugs until CI. Some config
  loaders (`tsconfig` `extends`, `typeRoots`) walk up too.
- Watchers, linters and editors in the main checkout would see every worktree unless every tool
  honours `.gitignore`; not all do. A sibling path is still predictable for harnesses.

Cost: harnesses must start in the worktree (they should anyway). `[worktree].root` may point
inside the repo; `doctor` then checks it is gitignored and warns about upward resolution.

## 6. Manifest and state

Per repo: `<main checkout>/.agents/worktrees.json`, the file other tools read (logbook, hull's
statusline, GUIs). Runtime state, never committed: hull ships the `.gitignore` line; without it,
`berth up` adds the path to `.git/info/exclude` (local, untracked) and reports `create`. berth
never edits tracked files.

```json
{ "version": 1, "repo": "/abs/hull", "worktrees": [ { "task": "fix-login",
  "branch": "berth/fix-login", "base": "a1b2c3d", "path": "/abs/hull.berth/fix-login",
  "state": "ready", "ports": { "start": 21340, "end": 21349, "named": { "web": 21340 } },
  "env": { "BERTH_TASK": "fix-login", "PORT": "21340" }, "copied": [ ".env" ],
  "setup": { "status": "ok", "secs": 14.2 }, "created_at": "2026-10-17T09:12:03Z",
  "owner": { "pid": 4242, "host": "laptop" } } ] }
```

With `[db]` an entry also has `"db": { "provider": "postgres", "name": "hull_fix_login",
"created": true, "host": "localhost", "port": 5432, "user": "dev" }`: `name` is the postgres
database, the compose project, or the sqlite path relative to the worktree; `created` is set
right after `createdb` succeeds (berth drops only what it created); `host`/`port`/`user` are
recorded for postgres only, so `down` works after config edits. Without `[db]` the field is
absent (an M1 manifest loads and is written back unchanged); never a URL or password.

`state`: `creating` → `ready` | `failed`; `removing`. `copied` lists paths only, never contents or
hashes. `env` holds only berth-derived and `[env]` values (§7), which are non-secret by rule.

Per user (ports are machine-wide): `$BERTH_STATE_DIR`, else `$XDG_STATE_HOME/berth`, else
`~/.local/state/berth` (also on macOS): `ports.json` = `{ "version": 1, "blocks": [ { "start",
"end", "repo", "task", "created_at" } ] }`.

## 7. Environment per worktree

`berth env` prints, and the manifest stores:

- `BERTH_TASK`, `BERTH_ROOT` (main checkout), `BERTH_WORKTREE`, `BERTH_PORT_BASE`,
  `BERTH_PORT_COUNT`, `PORT` (= base), one `<NAME>_PORT` per `[ports].names` entry.
- Caches, all inside the worktree (removed with it): `TURBO_CACHE_DIR=<wt>/.turbo/cache`,
  `VITE_CACHE_PATH=<wt>/node_modules/.vite/task-cache` (the variable hoard reads; confirm Vite+
  honours it at plan time), `CARGO_TARGET_DIR=<wt>/target` (a shared target dir serialises
  builds on cargo's lock), `TMPDIR=<wt>/.berth/tmp`.
- `[env]` values with placeholders `{repo}`, `{task}`, `{task_snake}`, `{port.<name>}`,
  `{port.base}`, `{worktree}`; with `[db]` also `{db.url}` (when a URL exists), `{db.name}`
  (postgres, compose) and `{db.path}` (sqlite). Unknown placeholder → exit 3.
- With `[db]`: the URL as `[db].env` (default `DATABASE_URL`; `""` exports none), and
  `COMPOSE_PROJECT_NAME=<project>` for compose. `[env]` cannot set either (§4).
- Default names, unique per checkout: `<h>` is the first 6 hex chars of FNV-1a (64 bit) over the
  bytes of the main checkout path, so two clones in directories of the same name do not share a
  database or compose project (`down` in one never touches the other's). Postgres:
  `<repo>_<h>_<task_snake>`, repo lowercased with other chars as `_`, cut so the whole name is at
  most 63 bytes (`db` if nothing is left). Compose: `<repo>-<h>-<task>`, repo lowercased with
  other chars as `-`, leading `-`/`_` trimmed, cut to 30. An explicit `[db].name`/`[db].project`
  is used as given (still validated). `createdb` failing with "already exists" prints
  `hint: if database <name> is a leftover, drop it with dropdb <name>, or set [db].name`.

- With `[cache] remote = "hoard"` and a running server (§12), `berth env` adds `TURBO_API` and
  `TURBO_TEAM=hoard`, read live (never stored in the manifest or `--json`). `sh` ends with
  `export TURBO_TOKEN="$(cat '<token file>')"` and `fish` with `set -gx TURBO_TOKEN (cat '<token
  file>')`, `pwsh` with `$env:TURBO_TOKEN = (Get-Content -Raw -LiteralPath '<token file>').Trim()`, so the token is never printed; `dotenv`/`json` omit it and say
  `note: TURBO_TOKEN is not printed; read it from <token file>` on stderr. Setup and teardown hooks
  get all three with the real token. No server: nothing exported, one `warning:` on stderr, exit 0.

Formats: `sh` (`export K='v'`, escaped), `fish`, `pwsh` (`$env:K = 'v'`: single quotes, `'` and
the typographic quotes U+2018/2019/201A/201B doubled; `$`, backtick, `"` and newlines are literal),
`dotenv`, `json`. Copied files' values are
never exported: they stay in `.env`, read by the app's own loader.

## 8. `berth up`, step by step

1. Validate task (2 on error), load config, resolve main checkout (3 on error). The main checkout
   is the first entry of `git worktree list --porcelain -z`. For a `--separate-git-dir` repo git
   lists the git dir itself first; then main is `git rev-parse --show-toplevel` run from the main
   checkout, and from a linked worktree of such a repo berth fails closed (exit 3, "run berth from
   the main checkout"). The common dir is `git rev-parse --git-common-dir`. The branch name is
   checked (`git check-ref-format --branch`, exit 2) and the base resolved (exit 3) here, before
   any lock, port or file. A manifest tracked by git (`git ls-files --error-unmatch
   .agents/worktrees.json`) is exit 3: "run git rm --cached .agents/worktrees.json".
   With `[db]`: the database name resolves (validation with a stand-in port block, always).
   For a task with no manifest entry yet (a lock-free peek; `up` on an existing task stays
   idempotent, §3) the provider's tools are also on `PATH` (`createdb` and `dropdb`, or `docker`;
   else exit 3) and, for sqlite, the template is a regular file without a pending `-wal` (else
   exit 3). All before any lock, port or file.
2. Take the repo lock `<git-common-dir>/berth.lock`. The user lock `<state>/ports.lock` is taken
   in step 4 only. Lock order is always repo → user, so two repos and two tasks cannot deadlock.
3. Check: task not present (else §3 idempotency; a `creating` entry whose owner is alive → exit 1
   "task is being created by pid N"), path free, `max` not reached, every `[copy]` match is
   ignored by git (`git check-ignore`) — a copy-listed file git would track is refused (exit 1:
   "add it to .gitignore"), because the agent would commit the secret. A sqlite target path git
   does not ignore is refused the same way. For an existing entry, `(provider, name)` from `[db]`
   must equal the recorded database (else `drift`: run `berth down <task>`); healthy is item
   `db <name> ok`. Nothing is created on any of these paths.
4. Under the user lock: allocate ports (§9) and write `ports.json`; release the user lock.
5. Still under the repo lock, in this order: add `/.berth/`, `/.agents/.*.tmp.*` and (unless git
   already ignores it) `/.agents/worktrees.json` to `<git-common-dir>/info/exclude`; write the
   manifest entry with `state: creating` and its owner; then `git worktree add -b <branch> <path>
   <base>`, or check out the branch if it exists. An existing branch records `base =
   merge-base(branch, resolved base)`, and a given `--base` is reported `skipped` with a warning
   on stderr. A failure in this step frees the ports and drops the entry. Release the repo lock.
6. Copy files (§10). Create `<wt>/.berth/tmp` (`/.berth/` is added to `.git/info/exclude`).
6a. With `[cache] turbo_seed = "link"|"copy"` (default `off`: nothing happens, no item), unlocked,
   after the copy: seed `<wt>/.turbo/cache` from `<main>/.turbo/cache` (§12). Never fails `up`:
   a problem is item `cache turbo skipped` plus a `warning:` on stderr.
6b. With `[db]`, unlocked, after the seed: create the database (§4): sqlite copies the template,
   postgres runs `createdb`, compose runs `compose up --detach --wait`. A refused item (sqlite)
   behaves like a copy refusal (rollback, exit 1). For postgres and compose, a short repo lock
   then sets `db.created = true` in the manifest; the process may die between `createdb` and that
   write, so `created` is false after such a crash and `down` never drops the database (it may be
   someone else's). The setup hook (step 7) sees `$DATABASE_URL` and the other §7 variables.
7. Run `[setup].run` in the worktree with the §7 env, output → stderr, with the timeout
   (`--no-setup` skips it).
   Hooks are POSIX `sh -c` commands on every OS. On Windows berth uses `sh.exe` from `PATH`, else
   Git for Windows' (`<prefix>\bin\sh.exe` or `<prefix>\usr\bin\sh.exe`, derived from
   `git --exec-path`); none found is exit 3 ("install Git for Windows"). Each hook runs in its
   own process group (Windows: its process tree); a timeout or failure kills the whole group
   (Windows: `taskkill /T /F /PID`). Ctrl-C does not reach the hook; if berth dies the hook keeps running and the entry shows
   `drift: interrupted`.
   A timeout fails the hook with status `timed out after Ns`; output readers are given 1 s to
   finish after the kill, then dropped, so a process that left the group cannot hang berth.
8. Retake the repo lock, set `state: ready` (or `failed`), release. A lock timeout here exits 3
   and says to run `berth down <task>` (setup finished; the entry stays `creating`).

On failure after step 5, berth rolls back (teardown, database drop if this run created it
(postgres: `createdb` succeeded; compose: `up` was attempted; `dropdb --force`, errors only warn),
worktree remove, branch delete if berth made it and it has no commits, ports freed) and exits 3 with the failed step and the last 20 hook-output
lines; `--keep-on-failure` keeps it as `failed` (database included, `created` as recorded) for `down`
later. Steps 6-7 run unlocked.

Atomic writes: temp file in the same directory, `fsync`, `rename`. Locks: `std::fs::File::lock`
(advisory; confirm the stable MSRV at plan time), waiting up to 30 s, then exit 3 naming the lock
holder recorded in the sidecar `<lock>.holder` (`pid=… host=… cmd=…`, written after the lock is
taken, emptied before release; a sidecar because Windows locks are mandatory and a second handle
cannot read the locked file). Process liveness and hostname are per OS (Windows: `tasklist`,
`COMPUTERNAME`). A `creating` entry whose owner pid is gone is shown by `ls`
and `doctor` as `drift: interrupted`; `down` cleans it.

## 9. Port allocation

- Blocks of `count` ports aligned to `count` inside `range` (default 1000 blocks of 10).
- Preferred block = FNV-1a 64 (implemented in-crate, frozen; never a std hasher, whose output may
  change) of `repo path + "\0" + task`, mod the number of blocks. Linear probe to the next block.
  Deterministic: the same task in the same repo usually gets the same ports across up/down cycles,
  which keeps bookmarks and OAuth callback URLs working.
- A block is free when no registry entry overlaps it **and** every port binds on `127.0.0.1` and,
  if IPv6 is available, `[::1]` (bind, then drop at once). This catches non-berth processes.
- A registry entry of another repo is stale when its repo path no longer exists, or when its task
  is not in that repo's manifest **and** that repo's lock is not held (an `up` there may be
  between writing ports and writing the manifest). The check is read-only and never blocks: an
  unreadable or invalid foreign manifest, or a path that is no longer a repo, is not stale.
  `doctor` reports stale entries. `up` treats them as free, and after the bind probe passes
  removes only the stale entries that overlap the block it chose; stale entries elsewhere stay.
- Entries of the repo running `up` whose task has no manifest entry are dropped under its repo
  lock (provably stale: they are left by a crash between steps 4 and 5).
- The probe is advisory: a process may take a port later; `ls` shows listening ports as
  information; a listening port is drift only when the task is not `ready` or its worktree
  directory is missing (`doctor`). berth kills nothing.
- No free block → exit 3 with "widen [ports].range or `berth down` unused tasks".

## 10. Copying untracked files

- Candidates come from the main checkout only: `git ls-files --others -z` (ignored and untracked),
  matched against `[copy].include` globs. Directories are not copied; symlinks are refused (a link
  could point anywhere); files over `max_bytes` are refused.
- Copy byte-for-byte (temp file + rename, mode kept, never via a string); an existing target file
  is left alone (`skipped`).
- Output names paths and counts only. Contents and hashes never reach stdout, stderr, `--json`,
  the manifest or logs. A test injects a canary value and asserts it appears nowhere (§13).
- berth cannot stop an agent reading `.env` in its worktree. Layered defence: berth never exposes
  values; the harness denies reads (hull's "never read `.env*`" plus a PreToolUse rule, §12).

## 11. `down`, `ls`, `env`, `doctor`

**down**: repo lock → `state: removing`. Refuse (exit 1, nothing changed) when the worktree has
staged or unstaged changes, or untracked files git does not ignore, unless `--force`. Copied files
are gitignored by rule (§8.3), so they never block. Commits are safe because the branch is kept;
`--delete-branch` deletes it only if it is merged into `base` (else refused, exit 1). "Merged"
means the branch is an ancestor of `base`, or, with git >= 2.38, `git merge-tree --write-tree
<base> <branch>` succeeds and yields base's own tree, i.e. every change of the branch is already
in `base` (squash and rebase merges). `merge-tree --write-tree` writes unreachable objects to
the object store (harmless; `git gc` prunes them), so the `gc` scan takes no lock and changes no
berth state but is not strictly write-free. berth never fetches: it compares with the local `base`, so
`git pull` first; with older git only the ancestor test applies. Then
teardown hook, the database drop (below), `git worktree remove` (`--force` passed through only with berth's `--force`),
`git worktree prune`, release ports, drop the manifest entry.
`down` refuses (exit 1) while a live process is still creating the task, and `--delete-branch`
resolves `[worktree].base` at that moment. An owner on another host cannot be verified and counts
as live; there the refusal (and `doctor`'s "cannot verify" item) names `berth down --force <task>`,
and `--force` skips this refusal. A live owner pid on this host refuses even with `--force` (it
may be a real running `up`). `down` only inspects, runs hooks in and removes a path that
`git worktree list` reports as a linked worktree; any other directory is left in place (item
`skipped`). A task without an entry just frees any orphaned port block. Lines `up` added to
`info/exclude` stay. A teardown hook may commit, so `--delete-branch` re-checks the merge state
right before `git branch -D`; if the branch gained commits it is kept (item `skipped`). If
`git worktree remove` fails without `--force` (e.g. the hook left untracked files), `down` exits 3
with a message that says to pass `--force`; the entry stays `removing`.
The database is dropped from the recorded `db` entry (config edits after `up` change nothing),
after teardown and before the worktree goes, with the worktree as cwd (the main checkout if it is
gone): postgres runs `dropdb --if-exists` only when `created` is true (else item `skipped`),
compose runs `compose down --volumes --remove-orphans` for the recorded project, sqlite needs
nothing (the file lives in the worktree). A failed drop exits 3 and keeps the entry `removing`
("fix it and run berth down <task> again, or pass --force"). `--force` adds `--force` to `dropdb`
and, if the drop still fails, warns and continues (item `db <name> drift`, "not dropped (ignored
by --force)").

**gc** `[--dry-run] [--delete-branch]`: removes finished tasks, each exactly as `berth down <task>`
(same locks, refusals, teardown and database drop; never `--force`; `--delete-branch` is passed
through and the branch is merged by definition). The scan takes no lock. A task is removed only
when it is `ready`, its path is a registered linked worktree, its branch exists and has at least
one commit beyond the recorded `base` (a fresh task is never "merged"), the branch is merged into
the resolved `[worktree].base` (the "merged" rule of `down`, so squash merges count only with git
>= 2.38; `git pull` first, berth never fetches) and the worktree is clean. Every other entry is
item `worktree <task> skipped` with `kept: state <s>`, `worktree missing`, `branch missing`, `no
commits yet`, `not merged into <base>` or `uncommitted changes`. `--dry-run` reports `ok` "would
remove" and changes nothing. No entries: item `gc repo ok` "nothing to collect". The report's
`worktrees` lists the entries removed (or that would be). Exit 0; 1 if a `down` refused (its
refusal items are included); 3 if a `down` failed (an `error <task>` item; the other tasks still
run). Removals run one after another in manifest order.

**ls**: one row per manifest entry plus unmanaged worktrees under `[worktree].root`
(`drift: unmanaged`): task, state, branch, ahead/behind, dirty, ports listening, path. Exit 0
(drift is reported, not fatal) or 3. Read-only, lock-free (atomic renames keep reads consistent).

**env**: prints §7 for `<task>`, or for the berth worktree containing the cwd. Read-only.

**doctor**: read-only checks, each an item: git present and new enough for `worktree add -b` and
`--porcelain -z` (minimum version decided at plan time); `berth.toml` valid; manifest vs
`git worktree list` consistent; every manifest path exists; copy patterns ignored by git;
manifest path ignored; worktree root not inside a tracked directory; stale registry entries;
reserved ports re-probed; `creating` entries with dead owners. Exit 1 on any drift, 3 on errors.
Lock-free and file-free like `ls`. Manifest, exclude, port and owner checks run only when
`.agents/worktrees.json` exists. A listening port is drift only when its task is not `ready` or
its worktree directory is gone.
With `[db]`: `db tools` is `drift` when `createdb`/`dropdb` (postgres) or `docker` (compose) is
not on `PATH` (with the install hint); sqlite gets one `db sqlite` item that is `drift` for a
missing or non-regular template, a non-empty `-wal`, or a target git does not ignore (never exit 3);
a `ready` postgres entry with `created: false` is `drift` ("database <n> was not recorded as
created; berth down will leave it"). With `turbo_seed` on and `.turbo/` not ignored: `cache turbo`
`drift`. With `remote = "hoard"`: `cache hoard ok` with the server URL, or `skipped` "no running
hoard server" (not drift, no warning, the token is never shown). `doctor` still takes no lock,
writes no file and never connects to a database or runs `createdb`, `dropdb` or `docker`.

## 12. Integration with hoard, hull, logbook, hoist

- **hoard.** Isolated caches start cold. Turborepo's remote cache is content-addressed and hoard's
  keys are immutable with if-not-exists writes, so many worktrees can share one remote safely. M2:
  `[cache] remote = "hoard"` makes `berth env` export `TURBO_API`/`TURBO_TOKEN`/`TURBO_TEAM` for a
  `hoard serve --storage fs` the user (or a hull hook) runs; berth does not start or supervise it
  (no daemon). berth reads `<main>/<hoard_dir>/port` (u16) and `token` (non-empty) live, since
  `hoard serve` writes them in its cwd (`./.hoard/`; default `hoard_dir = ".hoard"`) and may
  restart on another port; either missing or invalid means no server (§7 for the output rules).
  The token is a secret: only hook environments get its value, and `-v` never logs it. `[cache] turbo_seed = "link"|"copy"` (default `off`) seeds a new worktree's
  `.turbo/cache` from the main checkout's: top-level regular files only (no symlinks,
  subdirectories or `*.tmp`), existing targets kept; `link` hard-links and copies any file it cannot
  link, `copy` always copies (temp + rename). Skipped (item `skipped`) when `.turbo/` is not
  gitignored (the worktree would be dirty and `down` would refuse) or the source is missing.
  Hard links are safe because turbo writes archive, `-meta.json` and `-manifest.json` to a temp
  file and renames it, and eviction unlinks; no entry is rewritten in place (turbo `v2.11.8` and
  `main`, `crates/turborepo-cache/src/fs.rs`; older versions unchecked, use `copy`).
- **hull** (a later step, separate plan in hull): base renders `berth.toml` (seed), the
  `.agents/worktrees.json` ignore line, a `berth` skill (when to call `up`/`env`/`down`, read the
  JSON, never read copied files), a SessionStart hook snippet that exports `berth env` when the
  session starts inside a berth worktree (via the harness's env-file mechanism; confirm Claude
  Code's at plan time), a PreToolUse deny for `[copy]` paths, a statusline segment
  (task + `WEB_PORT`) read from the manifest, and one line in the generated `CLAUDE.md`. If a
  harness offers worktree-create/remove hooks, hull wires them to `berth up --json`/`down`.
- **logbook** joins agent spend to `task`, `branch` and `path` from the manifest.
- **hoist** releases berth (M3). **decisions**: D-00x for berth after dogfood.

## 13. Testing

- **Hermetic by rule.** Tests never touch the user's repos, real ports outside the test range, the
  network or `~`: `tests/common::berth()` sets `BERTH_STATE_DIR` and `HOME` to a temp dir,
  `GIT_CONFIG_GLOBAL` to an empty file, author env vars, `TMPDIR` to a writable dir, and a test
  port range unique per test process. No network or TLS crates exist in the dependency tree.
- **Temp git repos.** Fixtures build repos with `git init` in temp dirs: plain, pnpm-workspace
  shaped, with `.env` ignored, with `.env` tracked (refusal), with an existing branch.
- **Unit:** config and unknown keys; placeholders; slot hash (frozen vectors); allocation with a
  fake prober; manifest round-trip; env formats and shell escaping.
- **Integration (`tests/cli_*.rs`):** up → ls → env → down cycles; idempotent `up`; dirty `down`
  refused (exit 1) and `--force`; setup hook failure rolls back (exit 3); `--keep-on-failure`;
  `doctor` drift cases (deleted worktree dir, unmanaged worktree, stale registry, dead `creating`).
- **Concurrency:** spawn 8 `berth up` processes at once on one repo and on two repos; assert
  disjoint port blocks, 8 valid worktrees, a manifest that parses, and no leftover temp files.
  Kill a process mid-`up` (SIGKILL between steps via a test hook env var) and check recovery.
- **Port probing:** a test holds a listener in the preferred block; `up` must skip it.
- **Secrets:** a canary value in a copied `.env`; assert it is absent from stdout, stderr, JSON
  and the manifest across every command.
- **Snapshots:** insta for table and JSON output (never blind-accept).
- **Dogfood:** two agents in parallel on example-app (`pnpm run ci` + dev servers), then hull.

## 14. Repo layout

```
src/main.rs cli.rs lib.rs   clap (usage errors -> exit 2) -> run(Cli) -> Report -> table/JSON
src/error.rs report.rs      BerthError (usage 2, config/env 3); refusals are `Item`s with exit 1; Action, Report
src/config.rs               berth.toml (deny unknown fields), defaults, .worktreeinclude fallback
src/manifest.rs registry.rs .agents/worktrees.json; per-user ports.json; atomic writes
src/lock.rs ports.rs        repo/user locks with owner record; slot hash, block search, bind probe
src/git.rs copy.rs hooks.rs git CLI wrapper; allow-list copy; setup/teardown runner with timeout
src/env.rs  src/ops/        placeholders and formats; up down ls env doctor
tests/                      cli_*.rs, concurrency.rs, secrets.rs, common/mod.rs, snapshots/
```

## 15. Crates

All fetching-free; versions are **not** stated here and are pinned exactly in the M1 plan after
`cargo search`/crates.io checks, dated in `docs/PROGRESS.md`:

- `clap` (derive) — CLI. `serde`, `serde_json` — manifest and `--json`. `toml` — config with
  `deny_unknown_fields`. `thiserror` — error enum. `globset` not used: git matches the copy patterns via
  `ls-files --exclude`/`--exclude-from`. `wait-timeout` not used: std `try_wait` loop for hook timeouts.
- Dev: `tempfile`, `assert_cmd`, `predicates`, `insta` (json, yaml features as needed).
- Not used: `git2`/`gix` (git CLI, as in splice), any HTTP/TLS crate (no network calls, so the
  family's "rustls only" rule holds trivially), `dirs` (the state-dir rule is three lines).
- `#![forbid(unsafe_code)]`; `rust-toolchain.toml` pins hoist's toolchain (verify).

## 16. CI and release

- `.github/workflows/ci.yml`: one job, matrix `ubuntu-latest` + `macos-latest`: `cargo fmt --check`,
  `cargo clippy --all-targets --locked -- -D warnings`, `cargo test` (nextest if installed). Plus a
  `ci-ok` aggregate, the family pattern. No network in tests, so no secrets in CI.
- Release (M3) through `hoist release`: static musl Linux and macOS binaries on GitHub Releases,
  the npm launcher, `cargo install`; Windows (`win32-x64`, `--format pwsh`) in M3 too.

## 17. Facts to verify before the milestone that needs them

- Crate/npm names; minimum git version for the worktree and `-z` porcelain commands used.
- `std::fs::File::lock` stable version vs the pinned toolchain; behaviour on macOS APFS and NFS.
- Vite+: `VITE_CACHE_PATH` is honoured for the task cache; turbo: `TURBO_CACHE_DIR` relative vs
  absolute handling, and whether cache entries are ever rewritten in place (for M2 hard-linking).
- Verified 2026-10-17 (M1 dogfood, keelstackhq/example-app, Vite+ 1.1.0, turbo 2.11.8, pnpm 12.11.2):
  `TURBO_CACHE_DIR=<wt>/.turbo/cache` is honoured (63 entries after `pnpm run ci`, none in the main
  checkout). `VITE_CACHE_PATH` produced no `task-cache` directory: packages call `vp test/lint/...`
  directly (never `vp run`), so Vite+ writes no task cache; only vitest's per-package
  `node_modules/.vite` appears. Whether `VITE_CACHE_PATH` is honoured by `vp run` stays unverified.
  Two worktrees ran `pnpm run ci` (21/21 tasks, exit 0) and two `vp dev` servers on their own
  `WEB_PORT` at the same time.
- Verified 2026-10-21 (M2 plan and dogfood): turbo `crates/turborepo-cache/src/fs.rs` at `v2.11.8`
  and `main` writes every cache file (archive, `-meta.json`, `-manifest.json`) to a temp file and
  renames it; eviction unlinks. No entry is rewritten in place, so hard-linked seeds are safe.
  hoard (`ed8074a`): `hoard serve` writes `./.hoard/port` and `./.hoard/token` (random unless
  `--port`/`--token`); `hoard env` exports `TURBO_API=http://127.0.0.1:<port>`, `TURBO_TOKEN`,
  `TURBO_TEAM=hoard`. PostgreSQL 16.15: `createdb --template` waits 5 s for template sessions, then
  fails; names over 63 bytes are silently truncated; `dropdb --if-exists` is idempotent; `dropdb
  --force` ends sessions; two concurrent `createdb` from one template both succeed. Docker Compose
  v5.6.0: `--project-name P down --volumes --remove-orphans` works without the compose file and is
  idempotent; `up --detach --wait` waits for health; project names `^[a-z0-9][a-z0-9_-]*$`.
  git 2.38+ `merge-tree --write-tree` is used for squash-aware "merged".
- M2 dogfood 2026-10-21 (example-app, turbo 2.11.8, pnpm 12.11.2, PG 16.15, hoard from source):
  `up a`/`up b` at once, 1.3 s wall, 63 cache entries hard-linked into each, two databases from
  one template; `pnpm run check && pnpm run build` FULL TURBO in both at once (1.7 s vs 12.6 s cold
  in the main checkout); main `.turbo/cache` unchanged; an edit run in `a` uploaded 4 artifacts to
  hoard and `b` then hit them (18/18 cached); `gc` removed the merged task (database dropped) and
  kept the fresh one; after `down b` no task databases, no port blocks. sqlite and compose
  (Docker 29.9.1) smoke-tested on a scratch repo.
- Claude Code: `.worktreeinclude` format, worktree-create/remove hook names and payload, and the
  SessionStart env-file mechanism; Codex equivalents if any.

## 18. Milestones

1. **M1 — core** — `up`, `down`, `ls`, `env`, `doctor`; `berth.toml`; worktree, copy allow-list,
   port registry + probe, per-worktree caches and `[env]`, setup/teardown hooks, manifest, locks,
   rollback; tests as §13; CI on Linux + macOS. Dogfood: two agents on example-app.
2. **M2 — services** — DB providers behind `[db]`: `sqlite` (copy a template file), `postgres`
   (`createdb -T <template>` / `dropdb`, local server), `compose` (per-task `COMPOSE_PROJECT_NAME`
   and port mapping); hoard remote-cache export; turbo cache seeding; `berth gc` (remove `ready`
   tasks whose branch is merged and worktree clean). Done 2026-10-21, plan `2026-10-21-berth-m2.md`.
3. **M3 — distribution and integration** — release via hoist, npm launcher, Windows; hull renders
   `berth.toml`, skill, hooks and statusline segment; D-00x in `keelstackhq/decisions`.
4. **Later, with evidence** — hosted DB branches (network + token; maybe hoist's), cross-agent
   diff view for review, disk quotas.

Each milestone gets its own plan in `docs/superpowers/plans/`.
