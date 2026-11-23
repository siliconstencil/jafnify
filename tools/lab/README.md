# Network-namespace lab

Each script builds isolated network namespaces (`jl-*`), runs the real release
binaries (`linux/target/release/jafnify`,
`app/wgserv/target/release/jafnify-sharer-host`), prints a report and tears
everything down. Nothing is attached to the host network; the host's own
interfaces and default route are checked before and after.

Build both binaries in release mode first, then run a script as root (rootless
namespaces are blocked by AppArmor on many systems):

    sudo bash tools/lab/e2e-pair.sh

Output goes to `${LAB_OUT:-<repo>/tmp/lab}/out-*`, never system /tmp.

`LAB_HELPER`/`LAB_ROUTE` (used by `e2e-resilience.sh`) are swapped in only when
`/usr/libexec/jafnify` already exists. Run `jafnify install-helper` once;
otherwise the sandbox prints a warning and uses the pkexec fallback.

| Script | What it checks |
|---|---|
| `repro-solo.sh` | a lone joiner: no self-pairing, discovery frames leave on the LAN |
| `e2e-pair.sh` | code pairing, tunnel up, HTTP only reachable through the tunnel |
| `e2e-loss.sh` | sharer killed and restarted: loss detected, routing restored, reconnect |
| `e2e-known.sh` | pair, then reconnect with no code; stranger refused; forget |
| `e2e-dns.sh` | the gateway's DNS (10.233.0.1) answers through the tunnel |
| `e2e-link.sh` | netem loss, jitter and reorder; 20 sequential 1 MiB downloads; 60 s idle plus 5 GETs; MTU 1280 with a 4 MiB download and 1 MiB upload; a path-MTU black hole |
| `e2e-phonestats.sh` | the phone measurements channel: a paired joiner reads a fake summary; a LAN stranger gets no reply; the request rate is capped; a sharer with no provider gives no value |
| `e2e-hotspot.sh` | a phone with a cellular default route plus a hotspot LAN, and a laptop behind a default-deny firewall. Unpinned sharer refused, multi-interface sharer found, firewall named and fixed with the interface-scoped rule, PAKE `probe` both ways, cellular side isolated |
| `e2e-resilience.sh` | kill switch on and off, a link outage, peer loss and return, and restore of a deleted shared suppress rule |
| `e2e-ownership.sh` | two instances in one namespace; the second makes no privileged calls and the first stays reachable |
| `e2e-away-return.sh` | the laptop's Wi-Fi to the hotspot is gone for more than five minutes and comes back: reconnect with no user action, recorded as an away event and not a drop |
| `e2e-connsync.sh` | both clocks agree across a quick and a ten-second app update; a new app reports the update with no drop, an old app reports nothing, a reported crash still adds a drop |
| `e2e-multinic.sh` | the laptop has two networks; the search follows the hotspot across a longer absence and reconnects |
| `e2e-burst.sh` | burst UDP plus parallel TCP downloads; the tunnel stays up and the gateway does not stall |

`e2e-hotspot.sh` is written against the current behaviour. It never runs
`ufw`: the laptop firewall is an nftables table inside the `jl-jo` namespace
only.

`e2e-known.sh` keeps throwaway lab keys in its state dirs only during the run
(`KEEP=1` keeps them; delete them afterwards). `e2e-dns.sh` creates
`/etc/netns/jl-sh/hosts` for the run and removes it. Never point these scripts
at real keys or at `data/`.

`e2e-ownership.sh` takes `B=` for the second instance (defaults to
`linux/target/release/jafnify-test` when that build exists, else `jafnify`) and
`OWNERSHIP_SECONDS=` (default 60) for how long the second instance runs.
