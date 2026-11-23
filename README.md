# Jafnify

Jafnify shares a phone's internet connection with a computer over an encrypted
WireGuard tunnel. The phone runs a hotspot, the computer joins it, and once the
two are paired they find each other on their own: no account, no server, no
config files.

Jafnify is made by Silicon Stencil. The name is our own, inspired by Norse
mythology.

> **Use at your own risk.** Jafnify is free, open-source software provided
> "as is", without warranty of any kind, as stated in the MIT License. The
> authors and contributors are not liable for any damage, data loss, security
> incident, service interruption, charges, or legal consequences that result
> from using it.
>
> **You are responsible for how you use it.** Sharing a phone's connection may
> be limited or forbidden by your mobile plan's terms. Check them before use;
> any extra charges or account actions are your responsibility.
>
> **What it protects.** Jafnify encrypts the link between your computer and
> your phone.
>
> **Security.** Jafnify opens network ports on your devices while it runs.
> Only pair devices you own, on networks you trust. Keep both apps up to
> date. Report security problems privately as described in SECURITY.md.
>
> Do not use Jafnify for anything that is illegal where you are.

## Why a tunnel over the hotspot

A phone hotspot already carries the computer's traffic, but every connection
looks like a separate flow to the mobile network. Jafnify sends all of the
computer's IPv4 traffic to the phone as one WireGuard flow on UDP 9797, and the
phone forwards it through its normal mobile or Wi-Fi connection. The computer
also gets a stable encrypted link to the phone that survives a changing hotspot
address.

## What it is

- **Android app** (the sharer, the phone): a foreground service that runs a
  WireGuard gateway and a user-space router. The phone's internet leaves
  through its normal mobile or Wi-Fi connection.
- **Linux app** (the joiner, the computer): a command-line tool `jafnify`, a
  windowless tray process `jafnify tray`, and a window `jafnify-gui`. The tray
  process brings up a WireGuard interface `jafn0` and routes the computer's
  IPv4 traffic through the phone; the window is a short-lived client that talks
  to it over a local socket.
- **Shared code** (`jafnify-core`): pairing codes, discovery, the key exchange
  and the WireGuard config.

## Install (0.1.4)

Android (APK from the releases page; Android 5.0 / API 21 or newer, a recent
Android is recommended):

1. Download `jafnify-0.1.4-arm64.apk` and `SHA256SUMS` from the release page.
2. Verify the download: `sha256sum -c SHA256SUMS --ignore-missing` (expect
   `jafnify-0.1.4-arm64.apk: OK`).
3. Install on the phone (open the file, or `adb install
   jafnify-0.1.4-arm64.apk` after enabling installs from this source).
4. Allow notifications so the sharing notification (with Stop) shows.

Linux (x86_64 tarball or `.deb`): the app needs `wg-quick` (wireguard-tools),
`pkexec` (polkit) and `install`. Optional, for the kill switch, the inbound
guard and the DNS hooks: `iptables` or `nft`, and `resolvectl`
(systemd-resolved). Without a guard tool the tunnel still comes up and a
warning is printed.

Tarball:

1. Download `jafnify-0.1.4-linux-x86_64.tar.gz` and `SHA256SUMS`; verify the
   tarball the same way.
2. Extract it and run the installer from the extracted directory:

       ./jafnify-0.1.4-linux-x86_64/bin/jafnify install-helper

   It asks for a password once (pkexec) and installs `jafnify` and
   `jafnify-gui` in `/usr/local/bin`, the helper and `org.jafnify.policy` in
   their system paths, plus the desktop entry and the J icon.

`.deb` (Debian/Ubuntu):

    sudo apt install ./jafnify_0.1.4_amd64.deb

The package installs the same binaries under `/usr/bin`, the helper under
`/usr/libexec/jafnify`, and the polkit action, desktop entry and icons. Remove
it with `sudo apt remove jafnify`.

The desktop `Exec=` points at the installed GUI, so it works without a PATH
entry. The steps are chained; a failure stops and names the step. With the
helper, one user action makes one privileged call: connect installs the config
and runs `wg-quick up` in the same step. Without the helper, the app uses one
combined `pkexec` call as a fallback.

The default polkit action is `auth_admin_keep`. The first successful adoption
also installs a per-user rule at
`/etc/polkit-1/rules.d/49-jafnify.rules` that returns `polkit.Result.YES` for
`org.jafnify.tunnel` only when the adopting user is local and active. That is
the one password prompt; disconnect, reconnect, peer loss and return do not
prompt again. Other users, inactive sessions and remote sessions still get the
default `auth_admin_keep` prompt. `jafnify forget` and `jafnify uninstall`
remove the rule. The fixed helper, not the rule, remains the security boundary:
it accepts only its fixed verbs and validates every field.

The tray process lives next to the clock. It is windowless, so it works
natively on Wayland; the window is a separate process. Closing the window exits
only the window and keeps the tunnel managed. Right-click the tray for
Connect/Disconnect, Open and Exit (Exit disconnects first and ends both). The
window talks to the tray over a 0600 Unix socket in `$XDG_RUNTIME_DIR/jafnify`;
only one tray and one window run at a time.

The app writes events (state changes, peer found, the endpoint, reconnects with
reason and duration, errors, privileged calls and their result) with UTC
timestamps to `~/.local/state/jafnify/jafnify.log` and stderr. The log rotates
at 1 MB and keeps three files. Keys appear only as an 8-character prefix;
counters are never logged.

## Pair and connect

1. On the phone, start sharing. It shows a 4-digit code.
2. On the computer, connect the phone's hotspot, then run
   `jafnify pair --code 4321` (or use the app window and enter the four
   digits).
3. Both devices show "Connected" within a few seconds. Later, opening either
   app connects to the stored pair without a code.

The first time either program starts, it asks you to accept the terms above.
The CLI takes `--accept-terms` in scripts.

Useful commands on the computer:

    jafnify status      # pairing and tunnel state
    jafnify peers       # the stored device
    jafnify stop        # take the tunnel down
    jafnify probe --code 4321   # check discovery without a tunnel
    jafnify firewall rule       # the one firewall rule, if discovery is blocked

If the computer's firewall blocks discovery, Jafnify says so and offers one
narrow rule (UDP 9799, from your local network only). It never adds it without
asking. Remove it with the `ufw delete` command it prints.

## What the screens show

The phone and the laptop share one design. The home is a grid of 6, 7 or 8
tiles; choose the count and which tile sits in each slot, in order. A 7 grid
has one wide tile at the top or the bottom. Long-press a phone tile, or use
Settings, to open the layout screen. The tiles show signal, DNS, heat,
battery, speed, today and an average with the session peak. Tapping a tile
that sums over time switches between the session, today and the last day.

The phone's sharing card carries the pairing code with its expiry, New code
and Stop. Details show the path, range, speed, the phone measurements that
arrived inside the tunnel, and the connection log. Statistics shows the
session and daily totals, the drop history and the usage history. The network
report shows time on each network generation, switches, speed, drops, battery
drain and a signal history. Diagnostics exports a redacted local zip.

The laptop window shows the same home and details, a link test and the
connection log. The window has one fixed size, like a phone screen, so the
whole home always fits.

## Privacy

Jafnify has no account, no telemetry and no cloud service. The apps do not
send usage data anywhere.

- The paired key and the accepted terms live on the two devices. The Linux
  files are 0600; Android uses the app's private storage.
- Diagnostics stay on the device. The export is a local zip with peer names and
  key prefixes; it never contains a private key, and nothing uploads it.
- The phone and the computer find each other on the hotspot you already use.
  There is no rendezvous server and no third-party traffic.
- The project never carries traffic for anyone who is not the paired peer.

## Limits

- IPv4 only through the tunnel. IPv6 is blocked while connected.
- Linux builds are x86_64. Windows is not part of this release.
- The kill switch is opt-in and needs `iptables` or `nft`.
- The pairing code is 4 digits: pair in person, on a network you trust.
- One paired device at a time; forgetting is one-sided, so forget on both
  devices.
- The first pairing has no short-authentication-string check; see SECURITY.md
  for the accepted risk.
- Android 5.0 (API 21) or newer. The phone must be able to run a hotspot.

## Build from source

- Android debug build: `app/gradlew -p app assembleDebug` (APK under
  `app/app/build/outputs/apk/debug/`). Signed release builds:
  `tools/release/android.sh` with the keystore and passwords in the four
  `JAFNIFY_*` environment variables.
- Linux build: `cargo build --release --manifest-path linux/Cargo.toml` (gives
  `jafnify`, `jafnify-gui` and `jafnify-helper`). Release tarball and `.deb`:
  `tools/release/linux.sh`; checksums: `tools/release/sums.sh`.
- Rust tests: `cargo test --manifest-path jafnify-core/Cargo.toml` (also
  `linux`, `app/wgserv`).
- Android unit tests: `app/gradlew -p app :app:testDebugUnitTest`.
- Contributor notes: CONTRIBUTING.md; the network-namespace lab is described in
  `tools/lab/README.md`.
- Keys and signing material are never committed (see SECURITY.md).

## Support

Jafnify is free and open source. If it is useful to you, you can support the
work through GitHub Sponsors: <https://github.com/sponsors/siliconstencil>.

Donations are voluntary and do not unlock features. For bugs and security
reports, see the issue tracker and SECURITY.md.

## Thanks

Jafnify started from Vitaly Shukela's MIT-licensed wgslirpy and its WgServer
Android app; thank you. WireGuard is a registered trademark of Jason A. Donenfeld;
the Jafnify project is not affiliated with or endorsed by the WireGuard
project.
