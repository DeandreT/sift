# Releasing Sift

Two workflows separate ordinary distribution checks from access to a disposable
Azure namespace:

- **Desktop release** builds and verifies Windows and Linux archives. Manual
  runs upload preview artifacts. A matching version tag also creates or refreshes
  a draft pre-release, with write permission confined to that final job.
- **Live Service Bus validation** runs mutation-enabled integration tests on
  Windows and Linux only when manually requested with a configured namespace.
  A missing secret or unset mutation opt-in fails the job instead of silently
  reporting success from self-skipped tests.

Actions are pinned to immutable commits, checkout does not persist credentials,
and ordinary jobs have read-only repository access. This follows
[GitHub's workflow security guidance](https://docs.github.com/en/actions/reference/security/secure-use).

## Prepare and validate a preview

1. Finish the candidate implementation and update user documentation and
   `docs/release-notes.md`. Keep the application pre-1.0 until stable-readiness
   evidence is complete.
2. Run the normal format, Clippy, test, and build checks with the checked-in
   lockfile. Commit the candidate so its archive manifest identifies the
   tested source revision. Local archives record whether the source tree had
   uncommitted changes; use a clean committed tree for publication.
3. Run **Desktop release** on that revision. Download both workflow artifacts,
   verify their checksums, and complete the interactive checks in
   [release-validation.md](release-validation.md). CI window smoke checks do
   not certify keyring persistence or a real Azure connection.
4. Create a disposable Azure Service Bus namespace and store its SAS connection
   string as the `SIFT_TEST_SB_CONNECTION_STRING` secret in the repository's
   `live-validation` environment. Run **Live Service Bus validation** on the
   same revision with `allow_mutation` enabled. Its tests create unique
   `sift-test-*` names and clean up afterward; inspect for leftovers if a test
   is interrupted.
5. Complete interactive Entra and session checks on both supported desktop
   platforms and record links/results. The SAS CI job is not proof of Entra
   tenant selection or browser sign-in.
6. Resolve release blockers, rerun affected checks, and update the release
   validation record. Only mark a roadmap validation item done when its
   evidence exists.

## Create the downloadable preview

Set `[workspace.package].version` in `Cargo.toml` to the intended release
version, update `Cargo.lock`, and use a matching tag such as `v0.1.0`.

```sh
git tag -a v0.1.0 -m 'Sift 0.1.0 preview'
git push origin v0.1.0
```

The package job rejects mismatched tags, builds with `--locked`, checks the
actual executable format and architecture, and tests the extracted archive.
On success, the draft job attaches the archives and checksum sidecars to a
draft pre-release using the release notes in the tagged source. Re-running the
workflow can refresh a draft; it refuses to replace already published assets.

Review the draft's notes and recorded validation, then publish that pre-release
in GitHub. Creating the workflow files locally does not create a release;
repository push rights, working GitHub authentication, and a successful hosted
run are required. A workflow-dispatch run on a branch provides artifacts without
creating a GitHub release.

## Local archive checks

Python 3.12+ is required by the packaging scripts. Build a release binary for
the current host, then package and inspect it:

```sh
cargo build --locked --release -p sift
python3 scripts/package-release.py --binary target/release/sift \
  --target x86_64-unknown-linux-gnu
python3 scripts/smoke-release.py \
  target/release-artifacts/sift-0.1.0-x86_64-unknown-linux-gnu.tar.gz \
  --headless-only
```

On Windows use `target/release/sift.exe` and target
`x86_64-pc-windows-msvc`; the archive is a ZIP. Run without `--headless-only`
from a desktop to check the window too. A Linux CI host can use
`xvfb-run -a python3 scripts/smoke-release.py <archive>` with Xvfb,
`x11-utils`, and a software graphics driver installed.

The smoke check verifies both archive and member hashes, executes a harmless
legacy-import fixture, and checks an isolated empty configuration. It then
opens a desktop window, waits for it to remain open, and terminates the test
process. It does not connect to user namespaces or assert that a hosted keyring
is functioning. The package records the Git revision; archive bytes are
deterministic for identical inputs, but reproducible Rust builds across
toolchains are not claimed.

Build published Linux archives on the workflow's Ubuntu 22.04 runner to retain
its glibc baseline. A local build on a newer distribution can import newer
glibc symbols despite using the same Rust target name; label it as a developer
candidate and record its own runtime requirements. Local Arch Linux smoke
evidence does not replace Ubuntu or Windows release validation.

For isolated development or smoke checks, `SIFT_CONFIG_DIR` can point to an
absolute directory for Sift's `config.toml`. The script sets this explicitly
because Windows known-folder APIs do not honor `APPDATA` overrides. Normal
launches without that variable use the standard platform directory. This
override does not change the operating system credential store.

## Promote to 1.0

Complete every 1.0 gate in the validation record, including current Windows 11
and Ubuntu interactive evidence, both authentication methods, entity edits,
session lock renewal and loss, cleanup, and format migration checks. Keep macOS
excluded until it has its own build and validation coverage. Resolve blocking
issues before changing the application version to `1.0.0`.

After changing the version and release notes, build a fresh `v1.0.0` candidate
and verify its archives. The workflow still creates a draft pre-release so the
release cannot become stable by a tag push alone. After completing the final
review, publish it as a stable release and update the README/roadmap with the
actual release link and validation evidence.
