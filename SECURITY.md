# Security policy

## Use at your own risk

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
> date. Report security problems privately as described in this file.
>
> Do not use Jafnify for anything that is illegal where you are.

This file complements the MIT license; it is not legal advice.

## Reporting a vulnerability

Please do not open a public issue for a security problem. Use the repository's
private security advisory ("Report a vulnerability" on the project page), or
contact Silicon Stencil through the project profile. Include:

- the version and platform (Android or Linux),
- what you did, what happened, and why it matters,
- any logs or a diagnostics export with keys already redacted (never send a
  private key or a state file).

This is a volunteer project; we will acknowledge a report as soon as we can and
tell you what we plan to do.

## Supported versions

Security fixes go to the newest `0.1.x` release. Please reproduce on that
version before reporting.

## How Jafnify handles secrets

- Both apps generate a WireGuard keypair on the device during pairing. No key
  is typed, pasted or shipped with a release.
- The paired key is stored unencrypted at rest: in `state.toml` with mode 0600
  on Linux, and in the app's private storage on Android. Android backup is
  disabled, so the file is not uploaded by the system.
- Logs record only an 8-character prefix of a key, never the key itself.
- Diagnostics are a local zip with peer names and key prefixes. They never
  contain a private key and nothing uploads them.
- Release signing keys are never in the repository or in a build script. They
  exist only in the build environment for the duration of a release build.

## Accepted risk: the first pairing

The first pairing uses a 4-digit code. There is no short-authentication-string
(SAS) check, so an attacker who is already on the same hotspot and runs
purpose-built software during the first-pairing window can swap in its own key
and be stored as the paired device. That pairing then persists until you forget
the device, and every reconnect routes the computer's traffic
(`0.0.0.0/0`) to it.

This is a deliberate trade for a product used by one person with a laptop and a
phone side by side on their own hotspot. Pair in person and on a network you
trust. After pairing, reconnects need no code and a party that watched the
first pairing cannot reconnect later. Forgetting is one-sided: forget on both
devices.

## Scope

In scope: pairing and discovering a peer, the key exchange, the tunnel setup,
the Linux privilege boundary (the fixed helper and its polkit action), redaction
in diagnostics, and key storage on both platforms.

Out of scope: a device that is already rooted or compromised and has the stored
key, the security of the phone's own operating system, what happens to traffic
after it leaves the phone, and vulnerabilities in third-party dependencies
(report those upstream, and tell us so we can update).
