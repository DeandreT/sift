# Sift 0.1.1 preview candidate

This document describes the release candidate built from the accompanying
source revision. Publication and platform/live-service validation are tracked
in [release validation](release-validation.md). The application remains
pre-1.0 until those checks establish a stable release.

## Included workflows

- Multiple saved namespaces with SAS authentication, namespace management,
  queue/topic/subscription/rule views, and message inspection and settlement.
- Exact payload import/export, reusable message templates, scheduling,
  deferral, dead-letter recovery, and cancellable background operations.
- Namespace definition import/export and legacy connection-profile migration.
- Docked or detached entity views and a browser demo with simulated data.

The roadmap implementation adds Entra authentication through Azure CLI, entity
property editing, and retained session operations. These additions require the
feature-specific checks in the release validation record before publication.

Large JSON files and message bodies now use bounded text pages instead of
laying out the entire payload on every redraw. The regression suite covers
1.4 MB imports, repeated display frames, Unicode edits, and large pastes while
checking complete sends and template exports.

## Distribution

The **Desktop release** workflow builds x86_64 Windows and Linux archives with
SHA-256 sidecars and a source-revision manifest. It verifies the packaged
executable's headless startup, configuration write, and visible window before
uploading a workflow artifact. A version tag matching `Cargo.toml` also creates
a draft GitHub pre-release. The workflow does not publish a stable release.

See [installation](install.md), [platform support](support.md), and
[compatibility](compatibility.md). Archives are unsigned; macOS and ARM64
artifacts are not part of this preview. Linux requires a compatible desktop
graphics driver and runtime libraries. Credential persistence needs an
available operating system credential store.

The 0.1.1 build and Ubuntu installation instructions include the X11 keyboard
runtime library required to open the application window. This corrects a
missing prerequisite found while validating the 0.1.0 candidate.

## Upgrade

Close Sift and back up `config.toml` before replacing the portable executable.
Current config, namespace-export, and message-template formats are version 1.
Moving profiles to another computer requires re-entering SAS credentials or
signing in to Azure CLI again. See [migration](compatibility.md) for details.
