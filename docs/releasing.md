# Releasing

Versions, `CHANGELOG.md`, git tags and GitHub Releases are generated from [Conventional Commits](https://www.conventionalcommits.org/)
on `main` by [release-plz](https://release-plz.dev/). Merging a release PR also publishes the crate to crates.io through
Trusted Publishing, with no stored token ([Publishing to crates.io](#publishing-to-cratesio)). The very first upload
(`0.0.1`) was manual.
Status: the `CI` workflow has run on GitHub and is green. The `Release` workflow has run once, on a push to `main`:
it failed at the step that opens the release PR with "GitHub Actions is not permitted to create or approve pull
requests", which is the repository setting in the [checklist](#one-time-repository-setup-maintainer-checklist) below.
The rest of the release path (the pull request itself, the tag, the GitHub Release and the binary archives) has not
run yet; treat the first release as the test of it.

## How the version is computed

| Commit on `main` | Bump while `0.x` | Bump from `1.0` |
| --- | --- | --- |
| `fix:`, `perf:` | patch | patch |
| `feat:` | minor | minor |
| any type with `!` (`feat!:`) or a `BREAKING CHANGE:` footer | minor | major |
| `docs:`, `test:`, `chore:`, `ci:`, `style:`, `build:`, `refactor:` | none | none |

`release-plz.toml` sets `features_always_increment_minor = true`, so `feat` bumps the minor version even in `0.x`
(release-plz alone would bump only the patch). Only `feat`, `fix`, `perf` and `!` commits trigger a release; the
rest never open a release PR on their own, and `docs`/`test`/`chore`/`ci`/`style`/`build` are left out of the changelog.

**Baseline.** release-plz reads the current version from the crates.io index (`vtee 0.0.1` is there) and the git tag
`v0.0.1` marks where to start counting commits. Dry run of `release-plz update` (0.3.171) on a clone of `main` plus one
`feat` commit that touches a package file: `0.0.1 -> 0.1.0`, with a changelog entry under Added. Only commits that touch
files of the package count, so an empty `feat:` commit does not trigger a release.

**Minimum Rust version.** `rust-version = "1.89"`: the build and the whole Linux test suite pass on 1.89.0 and fail on 1.88.0,
because the locked `uuid 1.27.0` requires 1.89. Windows was only built on current stable. Raising it later is a normal
`chore:`; CI runs on stable only.

## Cutting a release

1. Merge work into `main` using Conventional Commit messages. Squash-merge PRs so the PR title is the commit message.
2. On each push to `main` the `Release` workflow opens or updates a PR titled `chore: release v…` with the version bump and the
   new `CHANGELOG.md` section.
3. Review that PR (the version, the changelog wording) and merge it.
4. The same workflow then publishes the crate to crates.io, creates the tag `vX.Y.Z` and the GitHub Release, and
   builds `vtee` for Linux x86_64 and Windows x86_64 and attaches the archives plus `.sha256` files. If the build
   fails, rerun it by hand with Actions > Release binaries > Run workflow > tag.

## With jj

- Write the commit message in Conventional Commit form: `jj describe -m "fix(session): reject names with ':' on Windows"`.
  Fix a wrong message before pushing with `jj describe -r <rev> -m ...`.
- To open a PR: `jj bookmark create my-change -r @-` then `jj git push --bookmark my-change`, and open the PR on GitHub.
  The `Conventional Commits` workflow checks the PR title and every commit message.
- Squash-merge on GitHub; after fetching, `jj git fetch` moves `main@origin`. Do not tag by hand.

## Forcing a version

- Edit `version` in `Cargo.toml` in the release PR branch (or use a `Release-As`-style manual edit); release-plz keeps a
  manually raised version.
- To make an unusual commit count as a release trigger, use `!` or a `BREAKING CHANGE:` footer.
- Tag format is `v{version}` (single-crate default).

## One-time repository setup (maintainer checklist)

- [ ] Settings > Actions > General > Workflow permissions: enable **Allow GitHub Actions to create and approve pull requests**.
- [ ] Optional but recommended: create a fine-grained personal access token (or GitHub App) with `contents: write` and
      `pull-requests: write` on this repo and store it as the secret `RELEASE_PLZ_TOKEN`. Without it, the release PR is
      opened by `GITHUB_TOKEN`, and GitHub does not run CI on PRs created that way.
- [ ] Settings > General > Pull Requests: allow squash merging (and make "Default to PR title" the squash message).
- [ ] Settings > Branches: protect `main` and require the `Conventional Commits` and CI checks if you want enforcement.
- [x] License: `MIT OR Apache-2.0` (the Rust convention), as `LICENSE-MIT`, `LICENSE-APACHE` and the `license` field
      in `Cargo.toml`.

## Publishing to crates.io

`0.0.1` is a preview that reserves the `vtee` crate name. crates.io needs an API token for the first upload of a
crate, so the first publish is done by hand by the owner:

1. On a clean checkout of the commit to release, with `Cargo.toml` at the version to publish:
   `cargo package --list` (check the file list and size), then `cargo publish --dry-run`.
2. `cargo publish`. The token comes from `cargo login`; use a short-lived token scoped to `publish-new` (add
   `publish-update` for later releases) and revoke it afterwards.
3. Tag the published commit: `jj tag set v0.0.1 -r <commit>` then `jj git push --tag v0.0.1`. release-plz computes
   the next version from the latest `vX.Y.Z` tag, so from `v0.0.1` the next `feat` proposes `0.1.0`. The `v0.0.0`
   baseline tag stays as history. The open release PR is recomputed on the next push to `main`; keep it unmerged
   until the release it describes is ready.
4. On crates.io the crate has a Trusted Publisher configured (GitHub, owner `huntwj`, repository `vtee`, workflow
   `release-plz.yml`, no environment). The API token used for step 2 is revoked.

### How later releases publish

Merging the release PR triggers the `release` job in `.github/workflows/release-plz.yml`. That job has
`permissions: id-token: write`. release-plz asks GitHub for an OIDC token for the job, exchanges it with crates.io for a
short-lived publish token (the same calls as `rust-lang/crates-io-auth-action`), runs `cargo publish`, then creates the
tag and the GitHub Release. Per release-plz's documentation, do not add the auth action and do not set a
`CARGO_REGISTRY_TOKEN` secret: either one makes release-plz use that token instead. `release-plz.toml` must not set
`git_only = true`, because git-only mode skips `cargo publish`; with it unset, release-plz reads the current version
from the crates.io index.

If the exchange fails (the job log shows an OIDC or 403 error from crates.io): check that the Trusted Publisher on
crates.io matches the repository and the workflow file name exactly (renaming `release-plz.yml` breaks it), and that
the job still has `id-token: write`. As a fallback, publish by hand as in steps 1 and 2 with a short-lived token, then
make sure the tag `vX.Y.Z` and the GitHub Release exist (re-running the failed job after a manual publish skips the
upload and creates them).

**Unverified.** The first real run of this path is the `0.1.0` release merge. Not yet exercised: the OIDC exchange
from the release-plz action, `cargo publish` from CI, and `release-plz release` end to end (it needs a GitHub token,
so it was not dry-run). What was checked locally with release-plz 0.3.171: `update` computes `0.0.1 -> 0.1.0` with and
without `git_only`, finds `vtee 0.0.1` on the registry without it, and `actionlint` passes on all workflows. The
re-run-after-manual-publish behaviour in the paragraph above comes from release-plz's documented design, not from a test.

`cargo package` ships the source, tests, fixtures and docs, about 1.1 MB over roughly 200 files. The `exclude` list
in `Cargo.toml` leaves out CI configuration, the Docker test image, `HANDOFF.md`, `release-plz.toml` and the oracle
generator for the differential test.

## Files

- `release-plz.toml`: release-plz settings and changelog grouping.
- `.github/workflows/release-plz.yml`: release PR, tag and GitHub Release.
- `.github/workflows/release-binaries.yml`: Linux and Windows archives; also runnable by hand.
- `.github/workflows/pr-title.yml` and `.commitlintrc.yml`: Conventional Commit checks.
- `CHANGELOG.md`: written by release-plz; do not edit released sections by hand.
