# Install Sift

Sift is distributed as a portable desktop application. A Rust installation is
only needed when building from source. See [support](support.md) for the platform
matrix and [compatibility](compatibility.md) before upgrading.

## Download

Use the assets attached to a published
[Sift release](https://github.com/DeandreT/sift/releases). Preview builds are
marked as pre-releases. If no release has been published yet, run the
**Desktop release** workflow in GitHub Actions and download its Windows or Linux
artifact. Workflow artifacts expire after 30 days and require repository access.
They are candidates for testing, not evidence of a published stable release.

Each download includes the executable, license, these instructions, support and
compatibility notes, release notes, and a `RELEASE.json` manifest. The matching
`.sha256` file verifies the archive. The manifest records the source revision,
whether local changes were present, and checksums of the packaged files.
These hashes detect file changes; archives are
currently unsigned.

## Windows

Download `sift-<version>-x86_64-pc-windows-msvc.zip` and its `.sha256` sidecar.
In PowerShell, compare the following hash with the first value in the sidecar:

```powershell
Get-FileHash .\sift-0.1.1-x86_64-pc-windows-msvc.zip -Algorithm SHA256
```

Extract the ZIP into a user-writable folder and open `sift.exe`. The portable
application does not require administrator rights or an installer. Windows may
identify the executable as an unsigned download; check the release source and
checksum before allowing it to run.

## Linux

Download `sift-<version>-x86_64-unknown-linux-gnu.tar.gz` and its `.sha256`
sidecar. In the download folder:

```sh
sha256sum --check sift-0.1.1-x86_64-unknown-linux-gnu.tar.gz.sha256
tar -xzf sift-0.1.1-x86_64-unknown-linux-gnu.tar.gz
cd sift-0.1.1-x86_64-unknown-linux-gnu
./sift
```

Linux archives from the release workflow target an x86_64 desktop with glibc
2.35 or newer. Locally built developer archives inherit the build machine's
library requirements and may require a newer distribution. On Ubuntu 22.04,
install the runtime libraries if they are missing:

```sh
sudo apt-get update
sudo apt-get install -y libgtk-3-0 libxkbcommon0 libxkbcommon-x11-0 libwayland-client0 \
  libxcb-shape0 libxcb-xfixes0 libegl1 libgl1 libvulkan1 libssl3 \
  libbrotli1 libzstd1 zlib1g \
  dbus-user-session gnome-keyring
```

On Ubuntu 24.04, use the same command with `libgtk-3-0t64` in place of
`libgtk-3-0` and `libssl3t64` in place of `libssl3`.

A desktop graphics driver and a user D-Bus session are required. Unlock the
desktop keyring to persist SAS credentials across launches. If Secret Service
is unavailable, Sift uses memory for that launch and asks for the secret again
after restart.

## Connect

Open **File > Connect** and choose the authentication method.

- **SAS:** name the profile and paste the namespace connection string. A
  `Manage` policy is needed for entity management; sending and receiving need
  the corresponding policy rights. Secrets are saved in the operating system
  credential store, outside `config.toml`.
- **Microsoft Entra ID:** install
  [Azure CLI](https://learn.microsoft.com/en-us/cli/azure/install-azure-cli)
  version 2.54 or newer, and ensure `az` is on the path available to Sift. Enter
  the namespace host, such as `orders.servicebus.windows.net`, and optionally
  the tenant ID. Use **Sign in and connect** for browser authentication.
  An existing Azure CLI sign-in can be reused. Sift obtains management and
  messaging tokens through the Azure SDK and holds its token cache in memory.
  The Azure CLI manages its own sign-in data outside Sift's configuration.

An Entra sign-in also needs Azure RBAC access to the namespace. **Azure Service
Bus Data Owner** provides full explorer access; **Data Sender** and **Data
Receiver** cover the respective messaging actions. An Azure subscription role
alone may not supply these data permissions. See Microsoft's
[Service Bus authentication documentation](https://learn.microsoft.com/en-us/azure/service-bus-messaging/authenticate-application).

Choose TCP or WebSockets for the transport. Mark **Auto-connect** if the saved
profile should reconnect on startup. A missing or expired sign-in requires
authentication again. Entra authentication targets Azure public cloud; use SAS
for the local Service Bus emulator.

## Upgrade or remove

Close Sift before replacing its executable with a new archive. Back up the
configuration file first; keep message templates and namespace exports you
want to retain. Replacing or removing the portable folder does not delete your
saved profiles or operating system credentials. See
[compatibility and migration](compatibility.md) for their locations and
downgrade precautions. Sift currently has no automatic updater.
