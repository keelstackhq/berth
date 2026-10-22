# Releasing berth

berth releases through [hoist](https://github.com/keelstackhq/hoist) (`hoist release`), the way
hoist releases itself. One release publishes, from one tag `v<version>`:

- five static binaries and `SHA256SUMS` on a GitHub Release (`berth-<target>[.exe]`),
- the npm launcher `@keelstackhq/berth` and the platform packages
  `@keelstackhq/berth-{linux-x64,linux-arm64,darwin-x64,darwin-arm64,win32-x64}`,
- the crate `keel-berth` on crates.io (`cargo install keel-berth`).

The files that define it: `hoist.toml`, `npm/berth/**` and `npm/berth-*/package.json`,
`pnpm-workspace.yaml`, `pnpm-lock.yaml`, `.github/workflows/release.yml`.
`tests/release_config.rs` keeps the versions and names in step.

## The gate

`release.yml` writes nothing unless the repository variable `BERTH_RELEASE` is `enabled`.

| Event | Variable unset | Variable `enabled` |
| ----- | -------------- | ------------------ |
| push to `main` | `detect` only | `detect`, then `build`, `publish` and `prepare` when a release is pending |
| `workflow_dispatch` | `detect` and `build` (dry run: 5 targets, `berth --version` smoke test, static check of the musl binaries; nothing published) | as push, plus the dry-run checks |

Merging to `main` therefore never tags, releases or publishes until the owner flips the variable.

## Normal flow (after the one-time setup)

1. Change the version in `Cargo.toml`, all six `npm/*/package.json` (and the launcher's
   `optionalDependencies`), then `pnpm install --lockfile-only`. `hoist release prepare` does this
   in a release PR (branch `hoist/release`); `tests/release_config.rs` fails when they disagree.
2. Merge to `main`. `release.yml` finds a version without a tag (`detect`), builds the five
   binaries (`build`), then `publish` (environment `release`, reviewers approve) creates the tag
   `v<version>` through the API, the GitHub Release, the npm packages and the crate.
3. `prepare` opens, updates or closes the next release PR.
4. A failed `publish` is safe to re-run from the Actions page: hoist checks each registry first.

Dry run any time: Actions -> release -> Run workflow (on a branch or on `main`).

## One-time setup (owner, in this order)

1. **hoist 0.1.0 on npm.** `@keelstackhq/hoist@0.1.0` must exist: `publish` and `prepare` install
   exactly that version (hoist's own first release is a human step in its repo).
2. **GitHub environment `release`** on `keelstackhq/berth`, with a deployment branch/tag policy
   (`main`) and required reviewers. Or let hoist create it: `hoist apply --area release`.
   A missing environment is created unprotected on first use, so do this before step 4.
3. **Optional secret `HOIST_RELEASE_TOKEN`** (fine-grained token, Contents and Pull requests:
   write) so the release PR runs CI. Without it `prepare` uses `GITHUB_TOKEN`.
4. **Repository variable `BERTH_RELEASE=enabled`.** The next push to `main` (or a re-run of
   `release`) tags `v0.1.0` and creates the GitHub Release; `publish` then stops at the first
   registry step, because the packages do not exist yet (a `first-publish:<pkg>` pending item).
5. **`hoist release bootstrap`**, locally, with your npm and crates.io logins: it publishes npm
   placeholders for the six packages, registers `npm trust github` for each (repo
   `keelstackhq/berth`, workflow `release.yml`, environment `release`), and publishes
   `keel-berth` 0.1.0 to crates.io from the tag. Run `hoist release bootstrap --dry-run` first.
6. **crates.io trusted publisher for `keel-berth`**: crates.io -> keel-berth -> Settings ->
   Trusted Publishing: repository `keelstackhq/berth`, workflow `release.yml`, environment
   `release`.
7. **Re-run the failed `publish` job** of the `v0.1.0` run (Actions -> release -> Re-run failed
   jobs). It now finds the tag, uploads what is missing and publishes the npm packages through
   OIDC. From here on every release is the normal flow.

Until then `npm i -g @keelstackhq/berth`, `cargo install keel-berth` and the GitHub Release
download do not work; build from source (`cargo install --path .`).
