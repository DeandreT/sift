# Compatibility and migration

Sift's application version and document schema versions are separate. The
current desktop application is pre-1.0. Within this preview, preserve a copy of
your data before upgrading or downgrading; broad backwards compatibility has
not yet been certified by cross-version release testing.

## Current formats

| Data | Version marker | Current version | Contents |
| --- | --- | --- | --- |
| Application configuration | TOML `schema_version` | 1 | UI/retry settings and namespace profiles; no SAS keys or access tokens |
| Namespace export | JSON `sift_export_version` | 1 | Queues, topics, subscriptions, rules, and their properties |
| Message template | JSON `format: "sift-message"`, `version` | 1 | Exact body and user-settable message metadata |
| Release package manifest | JSON `package_format_version` | 1 | Application version, target, revision, and packaged file hashes |

Message templates preserve binary data using base64. Broker delivery state,
lock tokens, and dead-letter state are not included in a reusable template.
Namespace exports do not transfer messages or secrets. Future unsupported
template, config, and namespace-export versions are rejected. Unsupported
namespace exports are refused before service operations; an unsupported
existing configuration is protected from overwriting even if startup falls
back to defaults. Early configurations without a schema marker retain the
version-1 defaults. Update Sift instead of manually changing a version field.

Correlation rules preserve XML Schema type metadata beside their property
values, including booleans, integer ranges, and decimal precision. Version-1
exports without this metadata remain accepted and treat their property values
as strings. Earlier development binaries did not preserve typed metadata when
rewriting rules or exports; keep original definitions when downgrading.
Unsupported types, null/structured values, or invalid supported values are
refused instead of being converted to strings. Import validates every rule
before creating any entities.

## Configuration and credentials

The configuration is written atomically to the platform configuration
directory:

- Linux: `$XDG_CONFIG_HOME/sift/config.toml`, usually
  `~/.config/sift/config.toml`.
- Windows: `%APPDATA%\DeandreT\sift\config\config.toml`.
- Experimental macOS source builds:
  `~/Library/Application Support/com.DeandreT.sift/config.toml`.

An explicitly configured `SIFT_CONFIG_DIR` overrides the configuration folder
for isolated development/test launches; it must be an absolute path. It does
not relocate operating system credentials or Azure CLI sign-in data.

SAS credentials live in the operating system credential store under service
`sift`, keyed by each profile's UUID and secret kind. Copying `config.toml` to a
different computer or user account does not copy those credentials. Re-enter
the connection string there. Entra profiles store the namespace and optional
tenant selection, while Azure CLI maintains sign-in state for the current
user. Reauthenticate on a different computer or user account.

Legacy `.NET` Service Bus Explorer XML configuration can be imported through
the File menu or `sift --import-legacy path/to/ServiceBusExplorer.exe.config`.
It migrates connection profiles into Sift's config and moves accepted secrets
into the operating system credential store. It does not import messages or
convert namespace exports. Check the import report for skipped profiles and
transport changes.

## Upgrade and downgrade procedure

1. Close every Sift instance and copy `config.toml` to a backup file. Keep the
   existing executable until the new version is verified.
2. Keep original namespace exports and message templates. Test a copy in the
   new version against a disposable namespace before importing definitions
   into a production namespace.
3. Replace the portable application folder, open Sift, and verify profile
   names, transport, authentication method, and credential retrieval.
4. For a downgrade, close Sift and restore the matching configuration backup
   along with the old executable. A backup only restores local settings;
   broker changes and message settlements remain in effect.

Avoid alternating application versions against the same configuration file.
Unrecognized fields can be lost when an older version saves configuration,
and format markers should not be rewritten to bypass validation.

## 1.0 compatibility policy

Before publishing 1.0, certify the current format versions with saved fixtures
and previous-preview data, document any migration, and record the supported
platform evidence. For stable 1.x releases, the intended policy is to keep
reading supported older formats, provide explicit migration when a schema
changes, and identify incompatible changes in release notes. These are release
criteria; they are not a claim that 1.0 has shipped or that every older preview
has already been validated.
