# Sift Roadmap

Sift is a native Azure Service Bus explorer. The feature work below is
implemented in the development preview; release publication still depends on
live Azure and supported-platform validation. Checked items have implementation
and local or linked hosted evidence. Open items require the external evidence
described in [release validation](release-validation.md).

Milestones remain in priority order without target dates. The current preview
candidate is 0.1.1, which supersedes the incomplete 0.1.0 release candidate.
The application remains pre-1.0 until release validation supports a stable release.

## Completed Foundation

- [x] Multiple namespace connections, saved profiles, startup auto-connect,
  and operating system credential storage.
- [x] Entity browsing, runtime counts, creation, deletion, status changes,
  and subscription rule management.
- [x] Message inspection, sending, receiving, settlement, scheduling,
  cancellation, deferral, and deferred retrieval.
- [x] Cancellable purge and dead-letter resubmission with progress reporting.
- [x] Namespace definition import/export, exact payload export, and reusable
  message templates.
- [x] Dockable and detached entity views, project documentation, and a browser
  demo using simulated data.

## Milestones

### 1 Downloadable Preview Release

- [x] Automate versioned Windows and Linux builds, archives, checksums, and
  packaged startup checks, with draft preview releases on matching version tags.
- [x] Prepare release notes, installation instructions, and known limitations.
- [x] Run the hosted Windows and Ubuntu release jobs for the 0.1.1 candidate;
  verify archives, checksums, packaged configuration/window startup, and draft
  release creation.
- [ ] Validate interactive installation, startup, credential storage, and
  connection setup on both supported desktop platforms.
- [ ] Validate core management and messaging against a disposable Azure Service
  Bus namespace using the gated live-validation workflow.
- [ ] Publish the validated preview artifacts and documentation on GitHub.

The 0.1.1 candidate passed hosted Windows and Linux CI and both platform release
jobs, including archive verification and packaged startup. The workflow created
a draft preview with both archives and checksum sidecars. Public publication,
live Azure checks, and interactive desktop validation remain open. The Linux
runtime fix and the incomplete 0.1.0 candidate remain documented in the
[validation record](release-validation.md), with run links and evidence limits.

### 2 Microsoft Entra ID Authentication

- [x] Add an Azure CLI sign-in flow with optional tenant selection, saved
  profiles, and transport selection alongside SAS authentication.
- [x] Authenticate management requests, AMQP messaging, and background message
  operations using a shared credential and renewable tokens.
- [x] Handle sign-in failures, insufficient permissions, timeouts, and stale
  reconnect attempts with clear feedback and redacted credentials.
- [x] Auto-connect saved Entra profiles and present sign-in when needed.
- [x] Add regression tests for token caching, renewal, endpoint validation,
  process timeouts, authentication headers, and redacted diagnostics.
- [ ] Exercise SAS and Entra with real accounts and namespaces, including
  renewal, revoked access, and both transports on supported platforms.

### 3 Full Entity Property Editing

- [x] Populate queue, topic, subscription, and rule edit forms from current
  properties, expose mutable fields, and label create-only settings.
- [x] Validate inputs and preserve settings the form does not expose.
- [x] Retain edits when validation or service requests fail.
- [x] Require explicit rule replacement and attempt to restore the original
  rule if replacement fails, reporting restoration failures clearly.
- [x] Refresh the entity tree and open views after successful saves.
- [x] Add property-preservation, validation, rule-recovery, and demo regressions.
- [ ] Run the opt-in live property-edit tests and interactive checks against a
  disposable namespace.

### 4 Session Message Settlement

- [x] Accept named or next available sessions and retain their receivers.
- [x] Receive under peek-lock and complete, abandon, defer, retrieve deferred,
  or dead-letter deliveries from a held session.
- [x] Track and renew the shared session lock, report expiry or ownership loss,
  and prevent delayed commands from acting on replacement receivers.
- [x] Release sessions explicitly, on view closure or navigation, on entity
  deletion, on disconnect, and on application shutdown with bounded cleanup.
- [x] Add lease, settlement, expiry, cleanup, and simulated-demo regressions.
- [ ] Validate actual broker settlement, lock loss, and cleanup using the
  opt-in session live tests and interactive supported-platform checks.

### 5 Version 1.0 Readiness

- [x] Define x86_64 Windows 11 and Ubuntu 22.04/24.04 preview support; document
  macOS as experimental and ARM64 as outside the release matrix.
- [x] Document compatibility and migration for configuration, namespace
  exports, and templates; reject unsupported formats without overwriting them.
- [x] Establish release evidence gates and a gated mutation-enabled live suite.
- [ ] Resolve issues found through supported-platform validation and preview use.
- [ ] Complete connections, auth, editing, messaging, sessions, credential-store,
  and migration checks on every supported platform.
- [ ] Gather preview-use evidence, validate the 1.0 candidate, and publish stable
  artifacts, release notes, and current documentation.

### Large JSON Responsiveness

- [x] Keep 1.4 MB JSON imports and message inspection responsive by limiting
  text layout to bounded pages and retaining the complete payload.
- [x] Avoid full-body clones, repeated Base64 detection, and expanded JSON
  formatting during redraws; cache decoded previews.
- [x] Preserve complete sends, exports, templates, and UTF-8 edits, including
  large pastes whose first-frame layout is also bounded.
- [x] Add 1.4 MB import, repeated-frame, Unicode paging, paste, and byte-fidelity
  regression cases.

## Maintaining This Roadmap

Update status only when implementation or required external evidence exists.
Keep the [README](../README.md), [architecture](architecture.md), and
[release validation record](release-validation.md) consistent with this file.
