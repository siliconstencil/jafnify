# Architecture

Jafnify has two apps and one shared Rust core. There is no server and no
account: the phone is the gateway, the computer is the client, and both find
each other on the hotspot the phone already provides.

## Topology

```
[Linux joiner]  -- UDP 9797 (WireGuard) -->  [Android sharer] --> mobile data
 jafn0 10.9.9.2        same LAN              gateway 10.9.9.1
        `----- multicast 239.255.42.99:9799 (discovery) -----'
```

- Sharer: a user-space BoringTun router on UDP 9797. Its internet goes out
  through whatever the phone uses (mobile data or Wi-Fi).
- Joiner: a kernel WireGuard interface `jafn0`, a full tunnel
  (`AllowedIPs = 0.0.0.0/0`), MTU 1280, keepalive 25 s, brought up with
  `wg-quick`.
- Joiner DNS: the config sets `DNS = 10.233.0.1`, the address where the
  sharer's gateway answers DNS and ping. Discovery and pairing happen on
  the LAN, by multicast and by unicast to a peer or gateway address.
- The tunnel subnet is `10.9.9.0/24` (sharer `10.9.9.1`, joiner `10.9.9.2`).
  The DNS address is separate so the gateway can answer it without a second
  interface.

## Components

```
jafnify-core (Rust, platform-agnostic)
  pairing      4-digit code, discovery token, code validator (TTL 600 s)
  discovery    multicast announce and receive on 239.255.42.99:9799
  pake         SPAKE2 pairing, replay cache, per-source rate limit
  keyexchange  X25519 key pair; low-order keys rejected
  known        pair secret, pair id, known-peer hello and proof
  link         "peer data stopped" detection
  orchestrator connection state machine, search and retry timing
  state        peer record and atomic 0600 state file
  config       wg-quick text generate and parse

linux/ (crate jafnify-linux)
  app          app state machine: interface detection, jafn0, loss detection,
               persist and forget, polkit denial, connected clock, counters
  ui_text      pure wording and formatting (twin of the Android UiText)
  view         pure state to text and controls
  root         privileged install, wg-quick up, wg-quick down
  daemon       windowless tray, log, and the 0600 control socket
  cli, bin/jafnify      status, peers, pair, forget, stop, tray
  bin/jafnify-gui       the egui window

app/wgserv (crate wgserv: cdylib libwgserv.so + rlib)
  joiner       join loop, known loop, gateway setup, status, pair persist
  lib          JNI entry points (Native_*) used by the Android app
  bin/jafnify-sharer-host   the sharer path on a Linux host, no JNI

app/app (Android, package com.siliconstencil.jafnify)
  MainActivity    tile home, pairing card, one primary button, Pair new
                  device / Forget device, Details
  SharerService   connectedDevice foreground service, multicast and wake
                  locks, Wi-Fi performance lock while serving, connected clock
  Native          JNI bridge to app/wgserv
```

`libwgslirpy-fork` is the vendored BoringTun and smoltcp router. Its `rx`
counter counts only datagrams the tunnel accepts, so LAN noise cannot fake
liveness.

## Wire frames

Setup frames use UDP port 9799, multicast to 239.255.42.99 with a unicast fallback to the peer or gateway address; the first byte is the type.

| Frame | Type | Purpose |
|---|---|---|
| discovery | 0x01 | "a Jafnify device is here"; a token proves nothing about the code |
| PAKE Hello | 0x05 | SPAKE2 share |
| PAKE Confirm | 0x06 | authenticated public key and UDP port |
| known Hello | 0x03 | "I know this pair" |
| known Proof | 0x03 | authenticated pair proof with the peer IPv4 |
| legacy | 0x02 / 0x04 | recognised only, to show the old-peer message |

Pairing is SPAKE2: the 4-digit code is the password, the joiner is side A and
the sharer side B. The derived key authenticates the peer's WireGuard public
key and UDP port. After pairing, the known-peer frames use a secret derived
from the two X25519 keys, not from the code, and carry no key or name.

## State files

Both platforms keep a `peers` list; each peer has an id, a display name, the
public key, the role, the last connected time, this pair's private key and an
endpoint hint. One known peer per device; a new pairing replaces it. Writes are
atomic.

| Where | Path | Notes |
|---|---|---|
| Linux | `~/.config/jafnify/state.toml` | mode 0600, directory 0700 |
| Android | `getFilesDir()/state.toml` | app-private, backup disabled |
| Host sharer | `--state-dir DIR` -> `DIR/state.toml` | 0600 |

The pair is written only after the first received WireGuard data of a code
pairing. Forget deletes the record and the key.

## Connection lifecycle

```
Idle -> Searching -> Pairing -> Connecting -> Connected
            |                       ^  |          |
            | known track: verified |  | 15 s no  | 35 s rx flat
            `-------- Proof --------'  | WG rx    v
                                       `-----> Retry --(10 s)--> Searching
```

- Searching: the code track announces every 1 s; the known track sends a hello
  every 1 s (every 5 s while connected).
- Pairing: a discovery frame from the other side; the code is accepted after
  3 s.
- Connecting: the PAKE exchange runs; the peer's Confirm gives its public key
  and port. Gives up after 15 s.
- Connected only after the first authenticated WireGuard data. A tunnel whose
  receive counter is flat for 35 s (above the 25 s keepalive) is lost.
- Retry: a fixed 10 s, no backoff, no cap.

## Not in this release

- No Android joiner screen; the Android app is the sharer only.
- No Windows or iOS client.
- IPv4 through the tunnel; IPv6 is blocked while connected.
