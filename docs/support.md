# Support and known limitations

Sift is a development preview. The table defines the intended desktop release
targets and their automation. A workflow definition does not by itself prove
that a particular release has passed on that platform; record results in the
[release validation checklist](release-validation.md).

| Platform | Release target | Automated coverage | Support boundary |
| --- | --- | --- | --- |
| Windows 11, x86_64 | `x86_64-pc-windows-msvc` ZIP | Windows Server 2022 build, tests, and packaged-window smoke check | Preview target; Windows 11 interactive validation required for each release |
| Ubuntu 22.04 / 24.04 desktop, x86_64 | `x86_64-unknown-linux-gnu` tar.gz | Ubuntu 22.04 build, tests, and packaged-window smoke check under Xvfb | Preview target; glibc 2.35+, graphics driver, and desktop session required |
| Other Linux distributions | None specifically certified | No distribution-specific CI | May work with compatible runtime libraries; report distribution and desktop details |
| macOS | No release archive | No CI | Experimental source build; excluded from the supported release matrix until validated |
| Windows/Linux ARM64 | No release archive | No CI | Not currently a release target |
| Browser demo | WebAssembly | Site build | Simulated namespace only; no desktop credentials or real Service Bus connection |

## Known limitations

- Preview archives are portable and unsigned. There is no installer, package
  manager integration, or automatic updater.
- Entra authentication depends on Azure CLI 2.54+ and its current sign-in.
  Sift does not currently offer a separate managed identity, application secret,
  or device-code provider. Azure public cloud is the Entra target.
- OS credential persistence depends on the local credential store being
  available and unlocked. The in-memory fallback forgets secrets on exit.
- Azure Service Bus tiers and entity settings determine which properties and
  messaging operations the service permits. A disabled entity or missing role
  can prevent operations even after a successful connection.
- A session lock can be lost because of timeout, network interruption, or
  broker-side ownership changes. Refresh or reconnect before retrying an
  action whose result is uncertain. Completed or deleted messages cannot be
  restored by Sift.
- Namespace exports transfer entity descriptions, not messages, credentials,
  or runtime counters. The legacy Service Bus Explorer XML file is supported
  for connection-profile import only.
- The management client uses Service Bus management API version `2021-05`.
  Pre-1.0 file formats and support guarantees are described in
  [compatibility](compatibility.md).

## Troubleshoot

If a Linux window does not open, start the executable from a desktop terminal
and inspect missing-library or graphics-adapter errors. A remote shell without
a display is not a desktop session. For automated checks use the documented
Xvfb smoke check in [releasing](releasing.md).

If a saved SAS profile asks for its connection string again, check the
credential-store backend shown in Sift's status bar. An in-memory backend means
the secret was not persisted. On Linux, check the user D-Bus session and unlock
the Secret Service keyring, then enter the connection string again.

If Entra authentication fails, run `az version` and `az account show` in a
terminal opened with the same user account as Sift. Sign in to the intended
tenant and reconnect. An authorization error after sign-in needs the appropriate
Service Bus role at the namespace or entity scope. Newly assigned roles may
take several minutes to become effective. Microsoft's
[authentication guide](https://learn.microsoft.com/en-us/azure/service-bus-messaging/authenticate-application)
describes role scopes.

For lock-lost, quota, disabled-entity, or transient connection errors, consult
Microsoft's [Service Bus error reference](https://learn.microsoft.com/en-us/azure/service-bus-messaging/service-bus-messaging-exceptions-latest).

## Report an issue

Open a [repository issue](https://github.com/DeandreT/sift/issues) with the Sift
version, archive target or source revision, operating system, transport,
authentication method, exact steps, and the relevant error from the log panel.
For session problems include whether a named or next available session was
accepted and which action lost ownership. Remove connection strings, SAS keys,
access tokens, message payloads, and business identifiers from attachments.
