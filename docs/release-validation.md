# Release validation record

This is the evidence gate for publishing a preview and later 1.0. Code or
workflow changes alone do not complete external validation. Fill in the
candidate revision and actual test results; retain links to hosted runs and
issues. Self-skipped live tests count as not run.

| Candidate | Value |
| --- | --- |
| Version | 0.1.0 development preview |
| Implementation revision | [08bf590](https://github.com/DeandreT/sift/commit/08bf5909b1df31ab9c991e40c31bc24061624a04); tagged release revision is recorded in its manifest |
| Windows archive checksum / workflow | Pending hosted build |
| Linux archive checksum / workflow | Pending hosted build |
| Disposable namespace | Pending authorized test credentials; omit secrets |
| Published preview | Pending hosted artifacts, Azure validation, and interactive platform checks |

## Local implementation evidence

These checks exercise the repository implementation. They do not certify the
supported Ubuntu/Windows release matrix or any Azure connection.

| Check | Local result |
| --- | --- |
| Host | Arch Linux x86_64; separate from supported-platform release evidence |
| Release workflow syntax | CI, Desktop release, and Live validation passed actionlint |
| Release scripts | Python syntax and archive layout/checksum/target/tag guards verified |
| Config compatibility | Legacy-profile defaults and unsupported-schema overwrite protection verified in unit tests |
| Graphical test environment | Official Arch Xvfb/xwininfo packages extracted under `/tmp`; private display startup verified without host installation |
| Actual packaged release smoke | Pending local release build and archive check |
| Local artifact ABI | Record the release binary's required glibc symbols; local Arch builds do not establish the Ubuntu 22.04 baseline |

## Local development evidence — 2026-10-02

The local host is Arch Linux x86_64 (kernel 7.2.7), using Rust 1.99.0.
This evidence covers the implementation candidate and does not replace the
supported Ubuntu/Windows or live Azure gates below.

- Workspace format, strict Clippy, native builds, and the WebAssembly demo
  build check pass.
- 128 offline unit, UI, simulator, and backend-bridge tests pass; doc tests pass.
  Nine Azure integration cases self-skipped because credentials were absent.
- The 1.4 MB JSON regression preserves exact sends/templates, bounds repeated
  composer/viewer layout, preserves UTF-8 page edits, and saves an entire large
  paste while bounding the paste frame. One diagnostic run measured about
  6 ms import, 113 ms for 20 composer frames, and 56 ms for 20 viewer frames.
  These are local test timings, not desktop frame-rate guarantees.
- Release-script guards, workflow syntax, checksums, and documentation links pass.
- Packaged development-binary headless startup/configuration and visible-window
  checks pass on a private Xvfb display with software graphics.
- The clean committed optimized binary was packaged and passed member/archive
  checksums, isolated headless startup/configuration, and visible-window smoke
  checks on the private Xvfb desktop. This developer archive was built from
  `08bf5909b1df31ab9c991e40c31bc24061624a04`; SHA-256:
  `c3f4725546503fae03517bb476a8f2920ebffbcac665c19334c2f75eef3b6b3c`.
  It is an Arch-built developer artifact, not the Ubuntu distribution binary;
  use the hosted Ubuntu 22.04 build for the supported Linux release.

## Automation

- [ ] Format, Clippy, unit tests, and build pass for the candidate on both
  Windows and Linux.
- [ ] Both versioned archives are generated with matching source/version
  metadata, license, documentation, and SHA-256 sidecars.
- [ ] The extracted Windows and Linux binaries pass headless configuration
  smoke checks and create visible desktop windows.
- [ ] Mutation-enabled live tests run against a disposable namespace on both
  Windows and Linux with no silently skipped credentials.

## Interactive platform validation

Run each item with the downloaded executable on Windows 11 and an Ubuntu
22.04 or 24.04 desktop. Record OS version, graphics/session type, artifact
checksum, result, and any issue link beside the item.

- [ ] Install/extract and open the application without a Rust installation.
- [ ] Save a SAS profile, restart Sift, retrieve its secret from the operating
  system store, and auto-connect successfully.
- [ ] Confirm unavailable/locked credential-store feedback and re-entry.
- [ ] Sign in with Entra ID, select the intended tenant, connect, disconnect,
  and reuse the saved profile at startup. Exercise token renewal and a revoked
  or insufficient role without exposing credentials in logs/configuration.
- [ ] Exercise both TCP and WebSockets connections and reconnect after a
  transient network interruption.
- [ ] Create/list/update/delete a queue, topic, subscription, and rule; verify
  mutable fields, create-only field presentation, preserved unedited fields,
  validation feedback, service errors, and rule replacement failures.
- [ ] Send/peek/receive/complete/abandon/defer/dead-letter messages; schedule
  and cancel; retrieve deferred messages; resubmit dead letters and cancel a
  background operation.
- [ ] Accept named and next available sessions; receive and settle messages;
  renew and lose locks; release explicitly and verify cleanup when the entity
  view closes, the namespace disconnects, and the application exits.
- [ ] Open a 1.4 MB JSON payload and inspect an equally large received body;
  page, edit, paste, send, copy, and export it while preserving complete bytes.
- [ ] Export/import namespace definitions and binary message templates,
  checking exact payload bytes and metadata.
- [ ] Open an existing version-1 config/template/export fixture and verify
  documented compatibility; try unsupported versions and inspect the errors.
- [ ] Import a legacy connection configuration and verify profile migration,
  secret persistence, repeated import, and skipped entries.

## Publication and stable readiness

- [ ] Resolve every release-blocking issue found in automation and desktop
  validation; link the fixes and affected rechecks.
- [ ] Review release notes, installation instructions, support matrix, and
  compatibility policy against the tested candidate.
- [ ] Publish the preview with both platform archives and checksum files;
  record its release URL above.
- [ ] Gather preview-use results and resolve remaining stable blockers.
- [ ] Repeat the supported-platform and compatibility checks on the 1.0
  candidate, update stable release notes, and publish fresh stable artifacts.

The working tree can complete implementation and prepare these gates without
cloud credentials. Leave a check open until its external evidence is recorded;
do not replace missing platform or Azure validation with a unit-test result.
