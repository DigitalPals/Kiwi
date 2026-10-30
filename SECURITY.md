# Security

Kiwi holds the keys to your apps: it signs with your Apple Development certificate, can
hold an App Store Connect key and installs software on your family's iPhones. Please report
security problems privately.

## Reporting

Use GitHub's **Report a vulnerability** (Security › Advisories) on this repository rather
than a public issue. Include what you found, how to reproduce it and what an attacker
could do with it. You'll get an answer within a week.

## The security model

- Kiwi listens on `127.0.0.1` only. `tailscale serve` publishes it to your tailnet over
  HTTPS. The installer refuses to run when Tailscale Funnel would put it on the internet.
- Tailscale identifies every device and person; members only reach the member endpoints
  and the apps the owner gave them.
- Requests must be addressed to Kiwi's own host name (DNS rebinding is refused with
  `421`), and every change needs `X-Kiwi: 1` with a JSON body, which blocks cross-site
  requests.
- Source and download URLs must be HTTPS and may not point at private or loopback
  addresses. IPAs and plists from sources are size- and depth-checked before they're read.
- Kiwi never asks for an Apple ID password. Signing uses your keychain through `codesign`;
  the optional App Store Connect key is stored with mode 0600 in Kiwi's data directory.
- Kiwi Builds keeps its App Store sign-in in ipatool's own keyring on the Mac; Kiwi's API
  and database never see it.

See [the architecture](docs/architecture.md#security) for details.
