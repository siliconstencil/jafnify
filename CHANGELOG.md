# Changelog

All notable changes to Jafnify are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.4] - 2026-11-23

### Added

- Sharing with the hotspot off now says "Hotspot is off. Turn on the hotspot
  to share." and offers a button to the hotspot settings; turning the hotspot
  on starts sharing by itself.
- A dismissible hint names the battery-optimisation exemption while sharing on
  battery, once per sharing run.
- Without the phone's network details permission, the SIGNAL tile says how to
  turn it on: "Turn on Network details in Settings" on the phone (a tap opens
  Settings) and "Turn on Network details on the phone" on the laptop.

### Changed

- The phone's sharing service is a `connectedDevice` foreground service. It
  matches the "relay internet to a paired device" use case and avoids the
  Android 15 `dataSync` time cap, and it holds a Wi-Fi performance lock while
  it serves.
- The laptop window has one fixed size, like a phone screen: the home, the
  tiles and the page buttons always fit, and the text is the embedded Inter
  font at whole-pixel sizes. Every page opens from a whole settings row.
- Empty tiles use the home captions, and each number on the laptop comes from
  one source.

### Fixed

- A phone app update brings a live sharing session back without opening the
  app. A user Stop still wins, and a session that was not running is never
  started.
- The pairing code sits in the card on the tile home with its expiry, New code
  and Stop, so a new phone can be paired without scrolling.
- The idle card says "Not sharing" once, and a cancelled start goes back to
  idle instead of saying "Starting..." for ever.
- The laptop status card shows no running time while not connected.
- A long tile line on the laptop ends in an ellipsis (full text on hover)
  instead of pushing the page buttons out of the window.
- Release binaries carry no local build paths.

## [0.1.3] - 2026-11-21

### Fixed

- The phone notices at once when the hotspot is turned off and tells you to
  turn it back on; the path, range and speed then stop showing the last values
  from the laptop.
- A phone app update is no longer counted as a drop or announced as a lost
  connection, also when the previous app never reported why it ended. A crash
  the phone reports still counts.
- The laptop and the phone show the same connected time: the phone's clock is
  the one both count from, and a short outage shows "Reconnecting..." instead
  of a time that keeps running.

### Changed

- A quick Stop and Connect can keep the same session; an outage longer than two
  minutes is recorded as a neutral "Phone away" event, and a stop on the phone
  is not counted as a drop.

## [0.1.2] - 2026-11-21

### Added

- A tile home on the laptop and the phone: signal, DNS, heat, battery, speed,
  today and an average with the session peak. Choose a grid of 6, 7 or 8 tiles
  and which tile sits in each slot, in order.
- A range tile on both screens: how far the laptop is from the phone, the link
  rate and a tone when the link is weak.
- Drop history and usage history: the newest drops with the phone context of
  the moment, and session and daily totals and speeds.
- Phone measurements on the laptop: network type, speed, battery, temperature
  and data totals, shared with the paired laptop inside the tunnel.
- A mobile data budget with a warning at 80%.
- A network report on the phone: time on each network generation, switches,
  speed, drops, drain of the same hours and a signal history.
- Desktop notifications for connected, lost and retrying, and a notice when
  the phone loses mobile data or drops from 5G to 4G.
- Connection details on both sides, and a local device name that is never sent
  to the other device.
- A link test (round trip and loss through the tunnel), a best-spot finder, a
  phone device panel and keep-screen-on.
- The phone lists the third-party licences it ships with.

### Changed

- Tiles on both screens show one window each, with a matching peak and one
  heat format.
- The search follows the hotspot when the laptop has more than one network,
  and logout cleans up its routing.
- The phone answers a restarted laptop in about a second instead of waiting
  for the retry.

### Fixed

- The phone's measurements keep updating after the phone app is updated or the
  session restarts.
- A "5G UW" / 5G advanced network is reported correctly, and the phone's
  signal level is shared with the laptop.
- A fault inside the phone's native layer is caught and reported instead of
  ending the app.
- The heat warning follows what Android reports; phones without a thermal
  status get a reworded warning.
- An hour of statistics is no longer lost when the app restarts in the middle
  of it.
- Removing the Linux package takes the tunnel down and leaves the host online;
  purging removes the key and the rule.

## [0.1.1] - 2026-11-10

Stability work on top of 0.1.0. No new features.

### Fixed

- Phone: sharing no longer ends when the hotspot goes away. The service waits
  ("Waiting for the hotspot") and resumes by itself when the hotspot is back.
- Pairing: a side that started first could leave the other one waiting for the
  key-exchange confirmation; the confirmation is now sent again a few times.
- Laptop: with the kill switch off, traffic falls back to the plain hotspot
  when the tunnel is lost and returns to the tunnel when it is back.
- Laptop: after the phone stops and restarts sharing, the tunnel is set up
  again with the new session instead of staying on the old one.
- Laptop: after walking out of hotspot range and back, the Wi-Fi network is
  found again within seconds, and the app waits quietly instead of cycling
  through retries.
- Laptop: an idle second instance no longer touches a tunnel it did not
  create.
- Laptop: an unreadable state file is kept aside instead of being overwritten,
  so a pairing is not lost; uninstall works for a `.deb` install.
- Phone: the Diagnostics list keeps its scroll position while it updates, and
  controls stay visible on vendor themes.

### Changed

- The discovery firewall rule is scoped to the interface and the port, so the
  direct copies get through as well.
- The connection log is kept across restarts on both sides.
- Phone: app data is excluded from cloud backup and device-to-device transfer.

## [0.1.0] - 2026-11-04

First working release: the Android sharer and the Linux joiner.

### Added

- Share one phone's internet connection with your own computer over an
  encrypted WireGuard tunnel. The phone runs the gateway; the computer joins
  the hotspot the phone already provides.
- Pair once with a 4-digit code. After that, both apps find each other and
  reconnect without a code.
- Android app: a foreground sharing service with a Stop action in the
  notification, optional start-on-open, optional resume after the system stops
  the app, and a Details view with session time, traffic counters and history.
- Linux app: command line (`jafnify`), a windowless tray process and a window.
  The tray shows the state (off, connecting, connected), offers Connect,
  Disconnect, Open and Exit, and can use an optional kill switch.
- Local diagnostics: a redacted export (zip) that stays on the device.
- Light and dark themes, with the same wording and states on both platforms.
- DNS and IPv6 handling: DNS goes through the tunnel and IPv6 is blocked
  while connected.

### Changed

- Pairing uses SPAKE2; the discovery token no longer carries the pairing code.
- The computer follows a changed hotspot address and rejoins without a new
  code. Discovery tries every usable local interface instead of only one.
- Failure text says what happened and what to do next; the pairing code stays
  on screen until you ask for a new one.

### Fixed

- A wait cycle between the router, the socket tasks and WireGuard could freeze
  the tunnel under burst load. The queues are acyclic now.
- Diagnostics redaction covers key material split by spaces or colons as well
  as contiguous keys.
- The Details screen opens at the top, and a retry countdown no longer hides
  the peer name.

### Security

- Keys are stored in the app's private storage on Android and in 0600 files on
  Linux. Logs record only an 8-character key prefix, never a key.
- The Linux kill switch is opt-in and off by default. Failures leave the
  existing rules in place rather than silently dropping them.
- No account, no telemetry, no cloud. Diagnostics are local and redacted.

### Notes

- Linux builds are for x86_64. The tunnel carries IPv4; IPv6 is blocked while
  connected.
- Windows and iOS clients are not part of this release.
- Jafnify works with WireGuard. WireGuard is a registered trademark of Jason A.
  Donenfeld; the Jafnify project is not affiliated with or endorsed by the
  WireGuard project.
