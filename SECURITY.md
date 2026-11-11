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

### The Mac builder

A Kiwi on Linux or in Docker builds iOS apps on a paired Mac
([Mac builder](docs/kiwi-builds.md#mac-builder)). What each side trusts:

- **The Mac connects out.** The builder opens no port; it calls Kiwi's Tailscale HTTPS
  address, verifying the certificate with the system's roots (plain HTTP only to a loopback
  address, for an SSH tunnel), and follows no redirects. Kiwi gets no shell or other access
  to the Mac.
- **The builder runs only its own code.** It runs the `runner.py` and `recipes.json` built
  into its own `kiwi` program, never code or commands from Kiwi. A task names a command
  (`plan`, `build` or `doctor`; never `setup` or `enroll`), one of the builder's own iOS
  recipes and a JSON request; anything else is refused. A Kiwi taken over could make the Mac
  build the apps of its recipes with your Apple account, nothing more.
- **Pairing needs the owner and a one-time code.** The code is made in Settings by the
  owner, has 60 bits, works once for 10 minutes, and five wrong codes in 10 minutes block
  pairing. The builder's token (256 bits) is stored on the Mac in a 0600 file; Kiwi keeps
  only its SHA-256. Pairing another Mac or removing it in Settings revokes the token.
- **A token alone is not enough.** Every builder request also needs `X-Kiwi: 1` and must
  pass Kiwi's identity rule: through `tailscale serve` from a device of the owner's
  Tailscale user, not Kiwi's own computer (or, only with `--trust-local`, a connection on
  Kiwi's computer). A leaked token is useless outside the owner's tailnet devices.
- **Kiwi doesn't trust what the Mac sends.** Events are checked as a local runner's are
  (known events, bounded sizes); the IPA must match the SHA-256 the build announced, and
  Kiwi then validates its bundle id, version and revision and signs it with its own
  identity before anything is published. The Apple account, ipatool's session and the
  decryption enrollment stay on the Mac; Kiwi's signing identity stays on Kiwi.

See [the architecture](docs/architecture.md#security) for details.
