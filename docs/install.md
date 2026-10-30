# Installing Kiwi

This guide takes you from a Mac without Kiwi to your first app installed on your iPhone.
It takes about half an hour, most of it in Apple's developer portal and waiting for builds.

1. [The Mac](#1-the-mac)
2. [Signing certificate and profile](#2-signing-certificate-and-profile)
3. [Tailscale](#3-tailscale)
4. [Build tools](#4-build-tools)
5. [Install Kiwi](#5-install-kiwi)
6. [Set up your iPhone](#6-set-up-your-iphone)
7. [Updating and removing Kiwi](#7-updating-and-removing-kiwi)

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
again and double-click it. If you would rather have Kiwi do this, give it an App Store
Connect key. See [Family](family.md#registering-iphones-with-apple).

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

Kiwi treats the Tailscale user who owns the Mac as its **owner**. If the Mac is
[tagged](https://tailscale.com/kb/1068/tags) (no owner), pass `--owner you@example.com` to
the installer.

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
git clone <this repository's URL> kiwi && cd kiwi
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
   needed once per iPhone. Kiwi shows these steps too.

Next: [Using Kiwi](using-kiwi.md), or share Kiwi with your [family](family.md).

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
