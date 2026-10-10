# Releasing

Versions, `CHANGELOG.md`, git tags and GitHub Releases are generated from [Conventional Commits](https://www.conventionalcommits.org/)
on `main` by [release-plz](https://release-plz.dev/). release-plz does not publish to crates.io: uploads are manual
for now ([Publishing to crates.io](#publishing-to-cratesio)).
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

**First release needs a baseline tag.** `Cargo.toml` is at `0.0.0`. With `git_only = true`,
release-plz finds the previous release from the latest `vX.Y.Z` git tag, so create `v0.0.0` on the first commit once and push
it: `jj tag set v0.0.0 -r 'root()+'` then `jj git push --tag v0.0.0`. Dry-run result with that tag (release-plz 0.3.170, on a
copy of the history): `0.0.0 -> 0.1.0`, with a changelog grouped into Added / Changed / Fixed. Without the tag the dry run also
proposed `0.1.0`, but that path is less clear, so create the tag. The release step itself (tag + GitHub Release) needs a GitHub
token and was not dry-run.

**Minimum Rust version.** `rust-version = "1.89"`: the build and the whole Linux test suite pass on 1.89.0 and fail on 1.88.0,
because the locked `uuid 1.27.0` requires 1.89. Windows was only built on current stable. Raising it later is a normal
`chore:`; CI runs on stable only.

## Cutting a release

1. Merge work into `main` using Conventional Commit messages. Squash-merge PRs so the PR title is the commit message.
2. On each push to `main` the `Release` workflow opens or updates a PR titled `chore: release v…` with the version bump and the
   new `CHANGELOG.md` section.
3. Review that PR (the version, the changelog wording) and merge it.
4. The same workflow then creates the tag `vX.Y.Z` and the GitHub Release, and builds `vtee` for Linux x86_64 and
   Windows x86_64 and attaches the archives plus `.sha256` files. If the build fails, rerun it by hand with
   Actions > Release binaries > Run workflow > tag.

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
4. On crates.io, open the crate's settings and add a Trusted Publisher (GitHub, owner `huntwj`, repository `vtee`,
   the release workflow's file name, optionally an environment). After that the workflow can publish without a
   stored token.
5. Only then turn publishing on in `release-plz.toml` (`publish = true` and drop `git_only`). **Unverified:** how
   release-plz behaves with trusted publishing and with a crate that has a `v0.0.1` tag made by hand. Try it with a
   dry run before relying on it.

`cargo package` ships the source, tests, fixtures and docs, about 1.1 MB over roughly 200 files. The `exclude` list
in `Cargo.toml` leaves out CI configuration, the Docker test image, `HANDOFF.md`, `release-plz.toml` and the oracle
generator for the differential test.

## Files

- `release-plz.toml`: release-plz settings and changelog grouping.
- `.github/workflows/release-plz.yml`: release PR, tag and GitHub Release.
- `.github/workflows/release-binaries.yml`: Linux and Windows archives; also runnable by hand.
- `.github/workflows/pr-title.yml` and `.commitlintrc.yml`: Conventional Commit checks.
- `CHANGELOG.md`: written by release-plz; do not edit released sections by hand.
