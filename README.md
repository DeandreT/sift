# sift

**sift** is a native Azure Service Bus explorer built with Rust and egui. It
keeps namespace structure, runtime counts, messages, dead-letter queues, and
long-running operations in one desktop workspace.

> sift is pre-1.0 software. The core management and messaging workflows are
> implemented, but releases and compatibility guarantees are not established
> yet.

See the [Roadmap](docs/roadmap.md) for planned features and release milestones.
See [Installation](docs/install.md), [Platform Support](docs/support.md), and
[Compatibility](docs/compatibility.md) for distribution and upgrade details.

## Highlights

- Connect to multiple Service Bus namespaces at the same time and optionally
  reconnect saved profiles at startup.
- Authenticate with SAS connection strings or Microsoft Entra ID through
  Azure CLI.
- Browse queues, topics, subscriptions, and rules in a searchable entity tree.
- Inspect entity configuration, status, runtime counters, and message counts.
- Peek or receive messages from queues, subscriptions, and dead-letter queues.
- Inspect JSON, XML, text, gzip, base64-wrapped, AMQP value, and binary bodies
  without discarding the original payload.
- Send, schedule, cancel, defer, retrieve, settle, resend, and dead-letter
  messages.
- Browse and edit large JSON payloads in bounded pages without changing the
  complete message.
- Open raw payload files in the sender, save exact message bodies, and reuse
  message metadata through versioned JSON templates.
- Browse session state, hold and renew session locks, and receive, settle,
  defer, retrieve, or dead-letter session messages.
- Create and delete entities, edit supported properties and status, and manage
  subscription rules with recovery when replacement fails.
- Purge an entity or resubmit an entire dead-letter queue with progress and
  cancellation.
- Export namespace entity definitions to JSON and import them into another
  namespace.
- Keep connection strings out of the application config by using the operating
  system's credential store.

## Quick Start

The workspace requires Rust 1.97 or newer. The checked-in toolchain file also
installs `rustfmt` and Clippy.

```sh
git clone https://github.com/DeandreT/sift
cd sift
cargo run -p sift
```

On Ubuntu and Debian, install the native windowing dependencies first:

```sh
sudo apt-get update
sudo apt-get install -y \
  libgtk-3-dev \
  libxkbcommon-dev \
  libwayland-dev \
  libxcb-shape0-dev \
  libxcb-xfixes0-dev
```

Windows and Linux are covered by CI. The desktop stack is cross-platform, but
macOS is not currently part of the CI matrix.

## Connect to a Namespace

Open **File > Connect**, name the profile, and paste an Azure Service Bus
connection string. The SAS policy must have the rights required by the
operations you intend to perform.

For Microsoft Entra ID, install Azure CLI 2.54 or newer and select **Microsoft
Entra ID** in the connection dialog. Enter the public Azure namespace host
(for example, `orders.servicebus.windows.net`) and optionally a tenant ID or
domain. **Sign in and connect** opens Azure CLI's browser sign-in flow;
**Connect** reuses the existing account. Full explorer operations require the
Azure Service Bus Data Owner role. Azure CLI manages persisted sign-in material;
Sift caches short-lived tokens in memory and renews them for management and
messaging operations. Startup auto-connect prompts for sign-in when needed.

Saved profiles contain display and transport settings only. Their connection
strings are stored in Windows Credential Manager, macOS Keychain, or the Linux
Secret Service. If the platform credential store is unavailable, sift falls
back to in-memory storage and forgets the secret when the process exits.

Both AMQP over TCP and AMQP over WebSockets are supported. Connection strings
for the local Azure Service Bus emulator are recognized through
`UseDevelopmentEmulator=true`.

## Common Workflows

The entity tree provides queue, topic, subscription, and rule actions through
its context menus. Opening a queue or subscription adds a dockable tab with
overview, messages, dead-letter, and session views as appropriate. Entity tabs
can also be detached into native windows.

Use **Edit…** in an entity overview to change supported settings.
Properties that can only be set at creation remain read-only. Rule edits
require replacement confirmation; if the new rule is rejected, Sift attempts
to restore the original and keeps the edits available.

The session view accepts the next available session or a named session and
retains its receiver while you work. Receive messages before settling them,
use renewal to extend locks, and use **Release session** when finished. Closing
the view, disconnecting, or exiting also releases held sessions.

Destructive operations are deliberately explicit:

- Receive-and-delete is labeled as destructive.
- Delete and purge actions use a confirmation dialog and can require the entity
  name to be typed.
- Purge and dead-letter resubmission run as cancellable background operations.
- Namespace exports include entity descriptions only, never messages, secrets,
  or runtime counters.

To import namespace profiles from a legacy Service Bus Explorer XML
configuration without opening the UI:

```sh
cargo run -p sift -- --import-legacy path/to/ServiceBusExplorer.exe.config
```

The same import is available from the **File** menu.

The sender can open text, JSON, XML, gzip, and binary payload files without
changing their bytes. Use **Open template** and **Save template** to move a
composed message between namespaces or keep it for later. The message inspector
can export either the exact body or a reusable `.sift-message.json` template;
templates include user-settable metadata and custom properties, never broker
lock or dead-letter state. Large text bodies use page controls to bound display
work; sending, copying, exporting, and saving templates include the full body.

## Workspace

| Crate | Responsibility |
| --- | --- |
| `crates/sift` | Native egui application, docked views, dialogs, and UI state |
| `crates/backend` | Dedicated Tokio runtime, command/event bridge, AMQP messaging, and background operations |
| `crates/core` | Configuration, connection parsing, SAS tokens, secret storage, body decoding, and shared message types |
| `crates/mgmt` | Azure Service Bus ATOM/XML management client and namespace import/export |
| `crates/web-demo` | WebAssembly entry point and deterministic in-memory Service Bus simulation |

See [Architecture](docs/architecture.md) for the runtime model, crate
boundaries, and end-to-end request flows.

## Development

Run the same checks as CI:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace
```

Live integration tests self-skip by default. To run them against a disposable
Azure Service Bus namespace:

```sh
export SIFT_TEST_SB_CONNECTION_STRING='Endpoint=sb://...'
export SIFT_TEST_SB_MUTATE=1
cargo test -p sift-mgmt --test live -- --nocapture
cargo test -p sift-backend --tests -- --nocapture
```

`SIFT_TEST_SB_MUTATE=1` enables tests that create uniquely named
`sift-test-*` entities, exercise management and messaging operations, and clean
them up afterward. Use an isolated namespace rather than production.

The backend includes property-edit, rule-recovery, and session-lifecycle live
scenarios. Entra validation uses `SIFT_TEST_ENTRA_NAMESPACE` and optional
`SIFT_TEST_ENTRA_TENANT` with an Azure CLI account; it exercises messaging when
the same mutation flag is enabled. Tests self-skip when their access is absent.
See [Release Validation](docs/release-validation.md) for the checks that require
actual desktop and cloud access.

## Website

The project site combines a statically exported Next.js application with the
real sift egui views compiled to WebAssembly. The embedded browser build uses a
deterministic in-memory namespace because browser sandboxes cannot use the
desktop AMQP and credential-store integrations.

Install Node.js 22, Trunk, and the WebAssembly Rust target, then build the
complete Pages artifact:

```sh
rustup target add wasm32-unknown-unknown
cargo install trunk --locked
npm ci
npm run build:site
python3 -m http.server 4175 --directory out
```

Open `http://localhost:4175`. The checked-in `Site` workflow builds the same
`out/` directory and is ready to publish it with GitHub Pages after the
repository is made public and Pages is configured to use GitHub Actions.

## Current Boundaries

- Entra ID uses the signed-in Azure CLI account and public Azure Service Bus
  namespace hosts. Azure CLI is required only for Entra authentication.
- Namespace transfer uses sift's versioned JSON format. It is not the legacy
  Service Bus Explorer XML format and does not transfer message data.
- The management client targets Azure Service Bus management API version
  `2021-05`.

## License

Licensed under the [MIT License](LICENSE).
