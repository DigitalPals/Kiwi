# Troubleshooting

Start with the log (`~/Library/Logs/Kiwi/kiwi.log` on a Mac, `journalctl -u kiwi` on Linux,
`docker compose logs kiwi` in Docker) and with `./install.sh --check`, which checks the whole
setup without changing anything.

## Installing Kiwi

**"No desktop session for …"**: Kiwi runs in your login session, so someone has to be
logged in to the Mac's desktop as you. Log in (or use Screen Sharing), then run the
installer again.

**"HTTPS certificates are off for your tailnet"**: turn on MagicDNS and HTTPS
Certificates on the [DNS page](https://login.tailscale.com/admin/dns) of the Tailscale
admin console.

**"This Mac is tagged in Tailscale, so it has no owner"**: tagged machines belong to no
user. Run `./install.sh --owner you@example.com` with your Tailscale login.

**"tailscale serve already maps / of … to …"**: something else is published at the root
of the Mac's Tailscale address. Remove it (`tailscale serve status`, then `tailscale serve
--https=443 --set-path=/ off`), or give Kiwi another address with `--public-url`.

**"Port 8742 is used by another program"**: stop that program or pick another port with
`--port`.

**The build fails**: make sure `cargo --version` and `node --version` (20 or later) work
in the same Terminal. Rust's first build downloads crates and takes a few minutes.

**"Kiwi … did not become healthy"**: the installer rolled back to the previous build. The
end of the log printed above says why. Fix it and run `./install.sh` again.

**"systemd is not running here"** (Linux): `install.sh` sets Kiwi up as a systemd service.
In a container or on a system without systemd, run Kiwi with [Docker](install.md#docker).

**"This computer is tagged in Tailscale, so it has no owner"** (Linux): as for a tagged Mac,
run `./install.sh --owner you@example.com`.

## Reaching Kiwi

**"Can't reach Kiwi" on the iPhone**: check that Tailscale is connected on the iPhone,
that the Mac is awake and logged in, and that Kiwi answers on the Mac:

```sh
curl -s http://127.0.0.1:8742/api/health
launchctl print gui/$(id -u)/local.kiwi | head -30
tailscale serve status
```

**"Couldn't identify this device through Tailscale"**: Kiwi couldn't ask tailscaled
who's connecting. Make sure Tailscale is running on the Mac; the log says whether Kiwi uses
the LocalAPI socket or the CLI, and why it failed. In Docker, Kiwi asks the Tailscale
container through the socket in the `tailscale-socket` volume: check that both containers
run (`docker compose ps`).

**"This host name is not served by Kiwi"** (`421 bad_host`) in Docker: Kiwi started before
the Tailscale container had its name (after a reboot or a Docker restart, Docker may start
Kiwi before the Tailscale container, before its socket even exists), so it only answers on
its listen address for now. The log says "no public URL (…): Kiwi answers only on … and
keeps asking". Kiwi asks Tailscale again every few seconds, then every 30 s, and logs
"public URL from Tailscale: https://…" when it has the name; from then on the address
works, without a restart. If that never comes, make sure the Tailscale container is
running and logged in (`docker compose ps`, `docker compose logs tailscale`). Setting
`KIWI_PUBLIC_URL` to the address avoids the lookup.

**"Kiwi is used through Tailscale, not on the computer it runs on"** (`403 not_owner`, on
Linux and in Docker): the request was made on Kiwi's computer itself, which anyone who
can log in there could have done (also through the computer's own `https://….ts.net`
address). Open Kiwi on your phone or another device in your tailnet. When that message
also reaches your phone, the log tells why:

- "ignored X-Forwarded-For from …: it comes from uid N": Kiwi trusts only tailscaled's
  connections, and expects tailscaled to run as root (uid 0). If uid N is tailscaled (it
  runs as another user, or in Docker user namespaces show the Tailscale container's root
  as another uid, often 65534), pass `--proxy-uid N` (`KIWI_PROXY_UID=N` in `.env` or the
  compose file). Don't do that when Kiwi shares the host's network
  ([With the host's Tailscale](install.md#with-the-hosts-tailscale)) and N is 65534: every
  user outside Kiwi's user namespace looks like that.
- "could not tell which user made the connection": Kiwi couldn't read
  `/proc/self/net/tcp` (or `tcp6`), or the connection wasn't in it. Kiwi must see the
  sockets of the network namespace `tailscale serve` connects from; don't hide `/proc/net`
  from it.

**Kiwi works in Safari but not from the Home Screen** (or the other way around): the
Home Screen app and Safari are the same device for Kiwi when Tailscale identifies them.
Delete the Home Screen icon and add it again from Safari.

## Signing

**Settings says "Can't sign apps"**: open Settings › Signing to see why:

- No identity: create an Apple Development certificate in Xcode › Settings › Accounts ›
  Manage Certificates, then **Rescan Profiles and Identities**.
- No usable profile: the profile must be a development profile, contain your certificate,
  belong to the same team and not be expired. See
  [Install](install.md#2-signing-certificate-and-profile).

**Signing fails with `errSecInternalComponent`**: `codesign` can't use the keychain.
That happens when Kiwi doesn't run in your desktop session (for example, started by hand
over SSH). Use the LaunchAgent the installer sets up, and make sure the login keychain is
unlocked, which logging in to the desktop does.

**"This Kiwi belongs to …"** (`403 not_owner`): the device is signed in to Tailscale as
someone other than the Mac's owner. Kiwi is only for the owner: sign in to Tailscale on the
device as them. When nobody owns the Mac (a tagged machine), run
`./install.sh --owner you@example.com`.

**"Kiwi doesn't know who owns it yet"**: its computer, or in Docker its Tailscale container,
is tagged, so it belongs to no Tailscale user. Run `./install.sh --owner you@example.com`,
or in Docker set `KIWI_OWNER` in `.env` and run `docker compose up -d`. A Tailscale container
that joined with a key without tags belongs to the key's creator, and Kiwi finds the owner
by itself.

## Installing apps

**iOS says "Unable to Install"**: usually the iPhone isn't in the profile, or the profile or
certificate has expired. Check Settings › Signing. Then make sure the app supports the
iPhone's iOS version (Settings › This iPhone shows the version Kiwi assumes).

**The app installs but won't open ("Developer Mode Required")**: turn on Developer Mode
once: Settings › Privacy & Security › Developer Mode, restart, then **Turn On**.

**The app opens but some features don't work**: the app's page lists entitlements under
**Removed When Signing**. A wildcard profile can't grant push notifications, App Groups,
iCloud and similar; extensions such as widgets and share sheets may not work.

**The install hangs**: iOS downloads the app from the Mac through Tailscale. Big apps take a
while on a slow connection. Keep Kiwi open; if it stays stuck, tap the app's button again.

**Kiwi shows Open for an app you deleted**: Kiwi can't see apps deleted on the iPhone unless
the iPhone is linked to the Mac (Settings › This iPhone). Open the app's page and choose
**•••** › **Reinstall**.

## Notifications

- Notifications only work in the Home Screen app, on iOS 16.4 or later.
- Turn them on in Kiwi under Settings › Notifications, allow them when iOS asks, and use
  **Send a Test**.
- If `vapid.json` was lost or replaced (the log says so), turn notifications on again on
  each iPhone.

## After a rollback

**"The database schema (version N) is newer than this Kiwi"**: an older build can't read a
database a newer Kiwi migrated. Install the newer build again, or restore the backup taken
before the upgrade, as described in [Operations](operations.md#roll-back).

## Kiwi Builds

See [Kiwi Builds](kiwi-builds.md). **Mac Setup** on the Kiwi Builds page shows what's
missing, and `runner.py doctor` checks the toolchain, the App Store sign-in and enrollment.

**"iOS apps can only be built on a Mac"** (`409 needs_mac`): Kiwi runs on Linux or in
Docker. Patched iOS apps are decrypted and built with Apple's tools on a Mac; Android apps
build anywhere. The Mac setup shows a single **macOS** check that is never ready.

**"Kiwi Builds for Android needs Python 3.12 or newer"**: Kiwi runs the Android builder with
an older Python. Install a newer one for the whole system and point Kiwi at it:
`./install.sh --python /usr/bin/python3.13`, or `KIWI_PYTHON`. The service can't use a
Python in a home directory (pyenv, uv, conda and the like). The Docker image has Python 3.13.

**"Kiwi couldn't prepare its Android app"** (Settings on an Android device): signing
[Kiwi's Android app](android-app.md) failed; the message says why, and
`automation/kiwi-app.log` in the data directory has the details. Kiwi keeps offering the
app it signed before and tries again within an hour. An APK in `android-app/unsigned/`
that is already signed (a debug build) is refused: place the unsigned release APK, or
remove the file to use the released app.

## Still stuck?

Open an issue with what you tried, the output of `./install.sh --check` and the relevant
lines of Kiwi's log. Remove addresses, logins and UDIDs you don't want to share.
