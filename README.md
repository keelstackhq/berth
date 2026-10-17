# berth

berth gives each parallel coding agent an isolated, ready workspace in a git repo: `berth up
<task>` creates a worktree, copies local env files, reserves a block of ports and runs your setup
hook, and `berth down <task>` cleans up. One static Rust binary, part of the keelstackhq family
(hull, splice, hoard, hoist). Status: M1 in progress. Build from source with `cargo build
--release` (Rust 1.97); the binary is `target/release/berth`. Design:
`docs/superpowers/specs/2026-10-17-berth-design.md`.

`berth down` keeps the branch (add `--delete-branch` to drop it once merged into `[worktree].base`;
this also deletes a branch you named with `--branch` when it is merged) and leaves the lines `berth up` added to `.git/info/exclude`; they are harmless.
