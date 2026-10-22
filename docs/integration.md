# Using berth from a coding harness

berth works with any harness that can run a command. Claude Code gets first-class wiring through
`berth hook`; other harnesses call `berth up <task> --json`. hull renders the files below into new
repos; this page is the reference for doing it by hand.

## Claude Code

### `.claude/settings.json`

```json
{
  "hooks": {
    "WorktreeCreate": [
      { "hooks": [{ "type": "command", "command": "berth hook worktree-create" }] }
    ],
    "WorktreeRemove": [
      { "hooks": [{ "type": "command", "command": "berth hook worktree-remove" }] }
    ],
    "SessionStart": [
      { "hooks": [{ "type": "command", "command": "berth hook session-start" }] }
    ]
  },
  "statusLine": { "type": "command", "command": "berth hook statusline" },
  "permissions": {
    "deny": [
      "Read(.env)",
      "Read(.env.local)",
      "Read(apps/*/.env.local)",
      "Read(.dev.vars)"
    ]
  }
}
```

If the repo already has hooks or a status line, add berth's entries next to them. Every hook
reads one JSON object on stdin (Claude Code's own payload; unknown fields are ignored) and needs
`berth` on `PATH`. Claude Code runs hooks in the project directory; berth takes the directory from
the payload, so `-C` is not needed.

| Event | What berth does |
| --- | --- |
| `WorktreeCreate` (`cwd`, `name`) | Runs `berth up <name>` from `cwd`. stdout is only the absolute worktree path; the table, logs and setup output go to stderr. Exit codes as `up` (1 refused, 2 bad name, 3 config or environment). |
| `WorktreeRemove` (`worktree_path`) | Finds the task whose path is `worktree_path` and runs `berth down <task>` without `--force`. A dirty worktree exits 1 and stays. An unknown path exits 0. |
| `SessionStart` (`cwd`) | Inside a berth worktree and with `CLAUDE_ENV_FILE` set, appends `# berth <task>` and `berth env --format sh` to that file, so every later Bash tool call has `PORT`, `WEB_PORT`, `DATABASE_URL` and the rest. Elsewhere it does nothing. |
| `statusLine` (`workspace.current_dir`) | Prints `<task> :<PORT>` inside a berth worktree, nothing elsewhere. Always exits 0, takes no lock. |

`berth hook` has no `--json` mode (`--json` is a usage error, exit 2): stdout belongs to Claude
Code's protocol.

Notes:

- `WorktreeCreate` replaces Claude Code's own `git worktree` step for `claude --worktree`,
  `EnterWorktree`, subagents with `isolation: worktree` and background sessions. All of them now get
  a berth worktree: ports, caches, database and setup hook included.
- Claude Code does **not** process `.worktreeinclude` when a `WorktreeCreate` hook exists. berth
  does the copying itself: `[copy] include` in `berth.toml`, else a `.worktreeinclude` file, else
  `.env`, `.env.local`, `apps/*/.env.local`, `.dev.vars`. Only gitignored files are copied.
- Claude Code never deletes branches when a worktree is removed, and neither does the hook: the
  branch stays. `berth gc --delete-branch` removes merged ones later.
- If `[setup]` takes longer than Claude Code's default hook timeout, raise the timeout of the
  `WorktreeCreate` hook (see Claude Code's hooks documentation).
- Paths with `.` or `..` segments are refused by Claude Code; berth prints a normalized path.
  Keep `[worktree].root` outside any symlinked directory below the repo.

### Status line with hull's `statusline.sh`

hull's `.claude/statusline.sh` already receives the status line JSON on stdin. To add the berth
segment, call the hook with the same input and append its output:

```sh
input=$(cat)
segment=$(printf '%s' "$input" | berth hook statusline 2>/dev/null)
# ... build the usual line, then:
[ -n "$segment" ] && line="$line | $segment"
```

Then point `statusLine.command` at `statusline.sh` instead of `berth hook statusline`.

### Deny reading copied secrets

`berth up` copies untracked, gitignored files (`.env` and friends) into each worktree. The agent
needs their effect, not their content: the environment is already exported by `SessionStart`.
List every `[copy]` path as `Read(...)` in `permissions.deny` (see the snippet above). Claude
Code also blocks Edit and Write for a path denied for Read. Keep the list equal to
`[copy] include`.

### `berth.toml` seed

```toml
version = 1

[worktree]
base = "main"

[copy]
include = [".env", ".env.local", "apps/*/.env.local", ".dev.vars"]

[ports]
names = { web = 0, api = 1, storybook = 2 }

[setup]
run = ["pnpm install --frozen-lockfile"]
timeout_secs = 900
```

`berth.toml` stays untracked or tracked as the repo prefers; berth never edits it. Everything is
optional (see the README for `[db]`, `[cache]`, `[env]`, `[teardown]`).

### One line for `CLAUDE.md`

```
Work in a berth worktree (`berth up <task> --json`, `berth down <task>` when done): use `$PORT` / `$<NAME>_PORT`, never fixed ports, and never read the files berth copied (.env*).
```

### The skill

Copy `skills/berth/SKILL.md` to `.claude/skills/berth/SKILL.md` (project skill). It tells the agent
when to call `up`, `env`, `ls`, `down` and `gc`, how to read the JSON and what the exit codes mean.

## Other harnesses (Codex, scripts, GUIs)

Anything that can run a command and parse JSON can use berth directly:

```sh
out=$(berth -C /path/to/repo up fix-login --json) || echo "exit $?: see items"
path=$(printf '%s' "$out" | jq -r '.worktrees[0].path')
```

- Run the agent with `path` as its working directory.
- Environment: `worktrees[0].env` (a JSON object), or `berth env fix-login --format dotenv|sh|json`.
  `PORT` and `<NAME>_PORT` are the ports to bind.
- stdout is exactly one JSON object (also for errors); logs and setup output go to stderr.
- Exit codes: `0` ok, `1` refused or drift (read `items`), `2` usage, `3` configuration or
  environment error.
- `berth up` is idempotent, so call it at every session start. Finish with `berth down <task>`;
  `berth gc` removes merged, clean tasks in bulk.
- Other tools can read `<repo>/.agents/worktrees.json` (version 1) for task, branch, path and
  ports. It never contains file contents or secrets.
