# Security

Kiwi holds the keys to your apps: it signs with your Apple Development certificate and
installs software on your iPhones. Please report security problems privately.

## Reporting

Use GitHub's **Report a vulnerability** (Security › Advisories) on this repository rather
than a public issue. Include what you found, how to reproduce it and what an attacker
could do with it. You'll get an answer within a week.

## The security model

- Kiwi listens on `127.0.0.1` only. `tailscale serve` publishes it to your tailnet over
  HTTPS. The installer refuses to run when Tailscale Funnel would put it on the internet.
- Tailscale identifies every device and its user. Only the Tailscale user who owns the Mac
  (or the one given with `--owner`) may use Kiwi; everyone else gets `403 not_owner`.
- On a Mac, Kiwi takes every program and user account on the Mac for its owner (that's
  how the browser on the Mac uses it). Run it on a Mac only you use; on Linux and in Docker
  only tailscaled may speak for a device.
- Requests must be addressed to Kiwi's own host name (DNS rebinding is refused with
  `421`), and every change needs `X-Kiwi: 1` with a JSON body, which blocks cross-site
  requests. A Content Security Policy and `X-Frame-Options: DENY` keep other sites from
  framing Kiwi, and install links stop working after 24 hours.
- The phone loads nothing from other sites: Kiwi fetches the artwork of the apps in your
  sources itself.
- Source and download URLs must be HTTPS and may not point at private or loopback
  addresses. IPAs and plists from sources are size- and depth-checked before they're read.
- Kiwi never asks for an Apple ID password. On a Mac, signing uses your keychain through
  `codesign`. On Linux, the uploaded `.p12` is kept with its password beside it (mode 0600,
  in a 0700 directory), so Kiwi can sign unattended: the files' permissions, not the
  `.p12` password, protect the key there.
- Kiwi Builds keeps its App Store sign-in in ipatool's own keyring on the Mac; Kiwi's API
  and database never see it.
- Kiwi Builds' Android patcher runs third-party patch code. It never receives the Android
  signing key or its password (Kiwi signs its output afterwards), it starts with a minimal
  environment, and on Linux and in Docker it runs in a Landlock and seccomp sandbox: no
  network, no access to Kiwi's data directory, writes only to its own job folder. On macOS
  there is no sandbox. See
  [How the patcher is contained](docs/kiwi-builds.md#how-the-patcher-is-contained).

See [the architecture](docs/architecture.md#security) for details.
