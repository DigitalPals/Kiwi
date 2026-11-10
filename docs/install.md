# Installing Kiwi

Kiwi runs on a computer that stays on, and your phones reach it through Tailscale:

| | |
|---|---|
| **A Mac** | Signs iOS apps with Apple's `codesign` and your keychain, can read a paired iPhone with `devicectl`, and builds patched iOS and Android apps with [Kiwi Builds](kiwi-builds.md). Follow the steps below. |
| **Linux** | A server, a home server or a [VM](#in-a-vm): [install.sh](#linux) sets Kiwi up as a systemd service, or run it in [Docker](#docker). Kiwi Builds builds Android apps there; patched iOS apps need a Mac. |

This guide takes you from a Mac without Kiwi to your first app installed on your iPhone.
It takes about half an hour, most of it in Apple's developer portal and waiting for builds.
On Linux, sections 2, 3 and 6 apply as well.

1. [The Mac](#1-the-mac)
2. [Signing certificate and profile](#2-signing-certificate-and-profile)
3. [Tailscale](#3-tailscale)
4. [Build tools](#4-build-tools)
5. [Install Kiwi](#5-install-kiwi)
6. [Set up your iPhone](#6-set-up-your-iphone)
7. [Updating and removing Kiwi](#7-updating-and-removing-kiwi)

On Linux: [Linux](#linux), [Docker](#docker), [In a VM](#in-a-vm).

## 1. The Mac

Kiwi runs on a Mac that stays on: installs and updates come from it.

- **Stay logged in.** Kiwi is a LaunchAgent in your login session, because `codesign`
  needs your login keychain. After a restart Kiwi starts once you log in. For a Mac
  without a screen, turn on automatic login (System Settings › Users & Groups), which
  requires FileVault to be off, or log in remotely with Screen Sharing after each restart.
- **Don't let it sleep.** System Settings › Energy (or Battery › Options): turn on
  **Prevent automatic sleeping when the display is off**.
- **Install Xcode** from the App Store and open it once to install its components. The
  command line tools alone (`xcode-select --install`) are enough for signing. Full Xcode
  adds `devicectl`, which lets Kiwi read the iOS version and apps of a paired iPhone, and is
  required for [Kiwi Builds](kiwi-builds.md).

## 2. Signing certificate and profile

Kiwi signs every app with your **Apple Development** certificate and a **development
provisioning profile**. iOS only installs an app when the profile lists the iPhone's
UDID, so each iPhone has to be in your developer account and in the profile.

You need an [Apple Developer Program](https://developer.apple.com/programs/) membership.
Free accounts can't create the wildcard profile Kiwi signs with.

### The certificate

1. Open Xcode › Settings › Accounts and add the Apple account of your developer team.
2. Select the team, click **Manage Certificates…**, then **+** › **Apple Development**.

The certificate and its private key are now in your login keychain. Check with:

```sh
security find-identity -v -p codesigning   # lists "Apple Development: Your Name (…)"
```

### Register your iPhone

You need the iPhone's UDID. Connect it to the Mac with a cable, select it in the Finder's
sidebar and click the line under its name until the UDID appears (right-click to copy it).
Xcode › Window › Devices and Simulators shows it too, as **Identifier**.

On [developer.apple.com](https://developer.apple.com/account/resources/devices/list),
open **Certificates, Identifiers & Profiles › Devices**, click **+** and add the UDID.
An account can register 100 iPhones a year.

### A wildcard development profile

A wildcard profile works for every app, so Kiwi can sign anything with it:

1. **Identifiers › +** › App IDs › App: description "Kiwi Wildcard", **Wildcard** bundle
   ID `*`. Continue and register.
2. **Profiles › +** › **iOS App Development**: choose the Kiwi Wildcard App ID, your
   Apple Development certificate and your iPhones. Name it, generate it, download it.
3. Double-click the downloaded `.mobileprovision`. It's copied to
   `~/Library/Developer/Xcode/UserData/Provisioning Profiles`, where Kiwi looks for it
   (as well as `~/Library/MobileDevice/Provisioning Profiles`).

When you add an iPhone later, edit the profile in the portal to include it, download it
again and double-click it.

> [!NOTE]
> A wildcard profile can't grant capabilities such as push notifications, App Groups or
> iCloud. Apps that need them install fine but lose those features. Kiwi lists them on the
> app's page, under Information, as **Removed When Signing**. If you have an explicit App ID
> profile for an app, Kiwi prefers it.

## 3. Tailscale

iPhones reach Kiwi through [Tailscale](https://tailscale.com), over HTTPS with a real
certificate, which iOS requires for over-the-air installs.

1. Install Tailscale on the Mac and sign in. Either the app from
   [tailscale.com/download](https://tailscale.com/download/mac) or Homebrew's open-source
   version works (`brew install tailscale`, then `sudo brew services start tailscale` and
   `sudo tailscale up`).
2. In the [admin console's DNS page](https://login.tailscale.com/admin/dns), turn on
   **MagicDNS** and, below it, **HTTPS Certificates**.
3. Install Tailscale on your iPhone and sign in to the same tailnet.

Kiwi is for one person, its **owner**: the Tailscale user who owns the Mac. Other people on
your tailnet can't use it. If the Mac is [tagged](https://tailscale.com/kb/1068/tags) (no
owner), pass `--owner you@example.com` to the installer.

## 4. Build tools

Kiwi is built from source on the Mac:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh    # Rust
brew install node                                                 # Node.js 20 or later
```

[Homebrew](https://brew.sh) is the easiest way to get Node.js; the installer from
[nodejs.org](https://nodejs.org) works too. `jq`, `sqlite3` and `curl` come with macOS.

## 5. Install Kiwi

Clone the repository and let the installer check the Mac first:

```sh
git clone https://github.com/DigitalPals/Kiwi.git kiwi && cd kiwi
./install.sh --check
```

`--check` changes nothing. It lists what's ready (✓), what's optional (-) and what needs
fixing (✗), and shows Kiwi's address. When it says **Ready to install**:

```sh
./install.sh
```

The installer asks for your password once (for the log rotation rule and `tailscale
serve`), builds Kiwi (a few minutes the first time), installs it and waits until it
answers. It finishes with:

```
==> Kiwi 0.2.0 is running at https://your-mac.your-tailnet.ts.net/ (tailnet only)
```

What it installed and where is listed in [Operations](operations.md). Options:

| Option | |
|---|---|
| `--check` | Only check the Mac and show the plan |
| `--public-url URL` | Use another HTTPS address than the Mac's Tailscale name |
| `--owner LOGIN` | The owner's Tailscale login (needed for a tagged Mac) |
| `--port PORT` | Loopback port (default 8742) |
| `--no-build` | Reinstall the last build without compiling |

## 6. Set up your iPhone

1. With Tailscale connected, open Kiwi's address in **Safari**.
2. Tap **Share › Add to Home Screen**, then open Kiwi from the Home Screen. Notifications
   and badges only work in the Home Screen app.
3. Open **Settings**. The card at the top should say **Ready to sign**; **Signing**
   shows the certificate and profiles Kiwi found. Turn on **Notifications** if you like.
4. Open **Sources**, tap **+** and paste an AltStore source URL (any repository's
   `apps.json`). Its apps appear in **Browse**.
5. Tap **Get** on an app, then confirm the install when iOS asks. The app appears on your
   Home Screen.
6. The first time you open an app Kiwi signed, iOS asks you to turn on **Developer Mode**:
   Settings › Privacy & Security › Developer Mode, restart, then **Turn On**. This is
   needed once per iPhone.

Next: [Using Kiwi](using-kiwi.md).

## 7. Updating and removing Kiwi

```sh
cd kiwi
git pull
./install.sh              # builds, backs up kiwi.db, installs, rolls back if the new build fails
```

```sh
./uninstall.sh            # removes Kiwi; keeps its data and logs
./uninstall.sh --purge    # also deletes the data and logs
```

Installed apps keep working after you uninstall Kiwi, until their signature expires.
More in [Operations](operations.md). If something goes wrong, see
[Troubleshooting](troubleshooting.md).

## Linux

Kiwi runs on a Linux computer with systemd, on x86-64 or ARM64. `install.sh` builds it from
source and installs it as a system service, much as on a Mac.

### What you need

- **Tailscale** from [its Linux packages](https://tailscale.com/download/linux), signed in
  (`sudo tailscale up`), with MagicDNS and HTTPS Certificates turned on for your tailnet
  (see [Tailscale](#3-tailscale)).
- **Tools**: `jq`, `sqlite3`, `curl`, `git` and a C compiler. On Debian and Ubuntu:
  `sudo apt install jq sqlite3 curl git build-essential`.
- **Rust** from [rustup.rs](https://rustup.rs) and **Node.js** 20 or later.
- **Python** 3.12 or newer, only for building Android apps with Kiwi Builds (Debian 13 and
  Ubuntu 24.04 have it as `python3`; elsewhere pass `--python /path/to/python3.13`).

### Install

As a user who may use `sudo` (or as root):

```sh
git clone https://github.com/DigitalPals/Kiwi.git kiwi && cd kiwi
./install.sh --check
./install.sh
```

The installer builds Kiwi as you, then installs it as root:

- the system service `kiwi.service` (from `deploy/kiwi.service.in`), running as its own
  user `kiwi`. It starts at boot without anyone logged in, its sandbox lets it write only
  its data directory, and the signing keys in that directory belong to the `kiwi` user
  alone. Linux has no login keychain, which is why Kiwi runs in your login session on a Mac;
- the program in `/opt/kiwi/app` (owned by root), its data in `/var/lib/kiwi`, its log in
  the journal (`journalctl -u kiwi`);
- `tailscale serve`: `/` of `https://<this computer>.<your tailnet>.ts.net` to Kiwi,
  tailnet only.

It takes the options of the [Mac installer](#5-install-kiwi), plus `--python PATH`. Servers
are often [tagged](https://tailscale.com/kb/1068/tags) in Tailscale, and then have no
owner: pass `--owner you@example.com`.

### Signing iOS apps on Linux

Kiwi signs on Linux with the same Apple Development certificate and development profile as
on a Mac ([section 2](#2-signing-certificate-and-profile)), but there is no keychain or
Xcode to find them in. Export the certificate with its private key as a `.p12` file (on a
Mac: Keychain Access, right-click the certificate › Export) and download the profile from
the developer portal, then upload both to Kiwi (`POST /api/signing/identity` and
`POST /api/signing/profiles`, see [the API](api.md)). Kiwi keeps them in its data
directory, under `signing/`. Without a Mac there is no `devicectl`, so Kiwi learns an
iPhone's iOS version from Safari and from installs.

### Updating and removing

The same commands as on a Mac: `git pull && ./install.sh` updates Kiwi (backing up
`kiwi.db` and rolling back if the new build doesn't start), `./uninstall.sh` removes the
service, the program and the serve mapping, and keeps `/var/lib/kiwi` and the `kiwi` user.
`./uninstall.sh --purge` deletes both, including Kiwi Builds' Android signing key: back it
up first ([Operations](operations.md#backups)).

## Docker

`compose.yaml` in the repository runs Kiwi next to a Tailscale container. The Tailscale
container joins your tailnet as its own machine and publishes Kiwi at
`https://<name>.<your tailnet>.ts.net` with `tailscale serve`, tailnet only. Kiwi shares
that container's network, so nothing is published on the Docker host, and asks its
tailscaled who each device is and what Kiwi's own address is.

1. Turn on MagicDNS and HTTPS Certificates for your tailnet ([Tailscale](#3-tailscale)).
2. Create an auth key in the Tailscale admin console under
   [Settings › Keys](https://login.tailscale.com/admin/settings/keys). It decides who owns
   Kiwi:
   - a key **without tags** makes the new machine yours, and Kiwi takes you as its owner;
   - a **tagged** key makes a machine that belongs to no user: then set `KIWI_OWNER` to your
     Tailscale login.

   You can also leave the key out and log in with the link that
   `docker compose logs tailscale` shows on the first start.
3. Clone Kiwi, fill in `.env` and start it:

   ```sh
   git clone https://github.com/DigitalPals/Kiwi.git kiwi && cd kiwi
   cp .env.example .env        # TS_AUTHKEY, TS_HOSTNAME (default kiwi), KIWI_OWNER
   docker compose up -d --build
   docker compose logs -f kiwi # "public URL from Tailscale: https://kiwi.<your tailnet>.ts.net"
   ```

4. Open that address on your phone ([section 6](#6-set-up-your-iphone)). iOS apps are signed
   as on [Linux](#signing-ios-apps-on-linux).

Kiwi's data is in the volume `kiwi-data` (`/data` in the container) and the Tailscale
machine's identity in `tailscale-state`. Keep both: without `tailscale-state` the container
joins the tailnet as a new machine, under a new name if the old one still exists, and
Kiwi's address changes with it. `.env` holds your auth key and is not committed.

To update: `git pull && docker compose up -d --build`. The container has no automatic
rollback like `install.sh`, so back up first ([Operations](operations.md#backups)).

- The image runs Kiwi as the user `kiwi` (uid 10001). For a host directory instead of the
  `kiwi-data` volume, give it to that user first: `sudo chown -R 10001:10001 ./data`.
- The App Store country starts as the region of `LANG` (`LANG=nl_NL.UTF-8`: the
  Netherlands), else the United States; Settings changes it.
- The image's healthcheck asks `/api/health`; `docker compose ps` shows it.

### With the host's Tailscale

When the Docker host already runs tailscaled, Kiwi can use it instead of a Tailscale
container: the host's network and the host's LocalAPI socket. Save this as
`compose.host.yaml` next to `compose.yaml`:

```yaml
services:
  kiwi:
    build: .
    network_mode: host
    environment:
      KIWI_OWNER: ""   # your Tailscale login when the host is tagged
    volumes:
      - kiwi-data:/data
      - /var/run/tailscale:/var/run/tailscale:ro
    stop_grace_period: 30s
    restart: unless-stopped
volumes:
  kiwi-data:
```

```sh
docker compose -f compose.host.yaml up -d --build
sudo tailscale serve --bg --https=443 http://127.0.0.1:8742
```

Kiwi then listens on the host's `127.0.0.1:8742` and answers at the host's own Tailscale
name. Its owner is the host's owner, or `KIWI_OWNER`.

## In a VM

A virtual machine is just another Linux computer: install Tailscale in the VM itself, then
use [install.sh](#linux) or [Docker](#docker) as above. The VM is a machine of its own on
your tailnet, with its own name, owner and tags.

- Give it 2 CPUs and 4 GB of memory for building Kiwi (2 GB is enough to run it), and
  10 GB of disk or more if you use Kiwi Builds.
- Start it with its host and keep it running: phones install and update apps from it.
- Don't run a copy of the VM next to the original. Both would use the same Tailscale
  identity and the same data. To move Kiwi, stop the old VM first.
