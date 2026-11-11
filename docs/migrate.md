# Moving Kiwi from a Mac to Linux

Kiwi can move from the Mac it runs on to Linux, for example a container on a Proxmox VE
server, and take everything with it: sources, devices, installed apps, settings, signed
apps, Kiwi Builds' apps and history, the Android signing key, the Web Push key and your
Apple signing identity. The Mac stays on as Kiwi's [Mac builder](kiwi-builds.md#mac-builder),
so Kiwi Builds keeps building iOS apps.

Two scripts do it:

- `./migrate.sh export` on the Mac packs Kiwi into one file, the **bundle**, stops Kiwi on
  the Mac and installs the Mac builder, already paired with the new Kiwi.
- `deploy/proxmox/kiwi-lxc.sh` on the Proxmox host creates Kiwi's container, installs Kiwi
  in it and imports the bundle. On any other Linux computer, install Kiwi with
  `./install.sh` and import the bundle with `./migrate.sh import`.

When the import is done, Kiwi runs on Linux with everything it had on the Mac, and the Mac
builder connects to it within a minute. Nothing needs setting up in Kiwi.

## What changes

- **Kiwi's address.** Kiwi gets a Tailscale name of its own, `kiwi` unless you pick another,
  so its address becomes `https://kiwi.<your tailnet>.ts.net`. On each phone, open the new
  address once:
  - On an iPhone, add it to the Home Screen again (Safari › Share › Add to Home Screen),
    delete the old icon, and turn notifications on again in Settings.
  - Kiwi's Android app can't reach the old address any more: tap **Change server** and enter
    the new one. Its apps stay as they are, because the new Kiwi signs with the same Android
    key.

  Apps Kiwi installed stay installed; Kiwi knows them, and updates them as before.
- **Signing.** On Linux Kiwi signs with a `.p12` copy of your Apple Development identity and
  the provisioning profiles Xcode kept ([Signing on Linux](signing-on-linux.md)); the export
  takes both from the Mac. Xcode renewed its profiles by itself, and nothing renews them on
  Linux. When the wildcard profile expires (**Settings › Signing** shows when) or you add an
  iPhone, download the profile again from developer.apple.com and upload it there.
- **iOS builds** run on the Mac builder. The Mac must stay on and logged in for them; while it
  is off, iOS builds wait and Android builds go on.

## Before you start

- **Proxmox VE 8.4 or later** (9 recommended), whose kernel (6.2 or later) lets community
  builds run sandboxed. Run `pveversion` on the host to check.
- **Disk space** for the bundle on the Mac and on the Proxmox host: about the size of Kiwi's
  data directory (`du -sh ~/Library/Application\ Support/Kiwi`), mostly downloaded and
  signed apps.
- **The new name** must be free in your tailnet. If an old device is called `kiwi`, remove
  it in the [admin console](https://login.tailscale.com/admin/machines) or pick another name
  with `--name`.
- **A Tailscale auth key** (optional): create one at
  [Settings › Keys](https://login.tailscale.com/admin/settings/keys), reusable or not, but
  not ephemeral and without tags, so the container belongs to you. Without one, the script
  prints a link to open instead.
- **On the Mac**, the repository at the latest version (`git pull`), and Rust, Homebrew's
  `python3.13` and Xcode, as for the [Mac builder](install.md#ios-apps-from-kiwi-builds-a-mac-builder).

## 1. Export on the Mac

On the Mac's desktop (or through Screen Sharing), in Terminal, in the repository:

```sh
./migrate.sh export
```

It checks the Mac and Tailscale, then:

1. builds the `kiwi` program for the Mac builder;
2. exports the signing identity Kiwi uses from the login keychain. macOS asks for your login
   password to allow it: choose **Allow**;
3. copies the provisioning profiles Xcode keeps;
4. asks you to confirm, then stops Kiwi on the Mac (and keeps it from starting at login). Its
   data directory stays as it is;
5. copies `kiwi.db` with SQLite's backup and packs it with the rest of the data directory into
   `~/kiwi-migration-<date>.tar`, with a SHA-256 checksum next to it;
6. installs the Mac builder (`local.kiwi-builder`), paired ahead of time with
   `https://kiwi.<your tailnet>.ts.net`, with the decryption enrollment and build tools Kiwi
   used on this Mac. It keeps trying until the new Kiwi answers.

Options: `--name NAME` for another Tailscale name, `--out FILE` for the bundle,
`--builder-name NAME` for the Mac's name in Kiwi, `--no-builder` to leave the Mac builder
out, `--identity SHA1` when the keychain has several code signing identities (`security
find-identity -v -p codesigning` lists them), `--p12 FILE` to use a `.p12` you exported
yourself. `./migrate.sh --help` lists them all.

> [!IMPORTANT]
> The bundle holds your signing keys (the Apple identity and Kiwi's Android key) and the
> Web Push key. Keep it private, copy it only to your own computers, and delete every copy
> once the new Kiwi runs.

## 2. Create the container on Proxmox

Copy the bundle and its checksum to the Proxmox host, and the script with it:

```sh
scp ~/kiwi-migration-<date>.tar ~/kiwi-migration-<date>.tar.sha256 \
    deploy/proxmox/kiwi-lxc.sh root@proxmox:/root/
```

On the Proxmox host, as root:

```sh
./kiwi-lxc.sh --bundle /root/kiwi-migration-<date>.tar --authkey-file /root/tailscale.key
```

It checks the bundle's checksum, then:

1. creates an unprivileged Debian 13 container named after the bundle (`kiwi`) that starts
   with the host: 4 cores, 8 GB of memory, a 64 GB disk, DHCP on `vmbr0`, with nesting on
   (Kiwi's service runs sandboxed, and so do community builds) and `/dev/net/tun` for
   Tailscale;
2. installs Tailscale, Rust, Node.js and Python, and signs Tailscale in with the auth key
   (or prints a link), then checks that the container got the bundle's name;
3. clones Kiwi from GitHub into `/root/kiwi`, builds it and installs it with `./install.sh`,
   with the bundle's owner (`--owner`);
4. streams the bundle into the container and imports it with `./migrate.sh import`;
5. checks that Kiwi answers at its address.

Building takes about ten minutes. Options: `--ctid`, `--storage`, `--disk`, `--cores`,
`--memory`, `--bridge`, `--ip CIDR --gw IP` for a fixed address, `--vlan`, and `--ref` for a
branch or tag other than `main` (`./kiwi-lxc.sh --help`). If a step fails, the script stops
and leaves the container for you to look at (`pct enter <id>`); remove it with `pct destroy`
before you run the script again.

Kiwi in the container is updated like on any Linux computer:

```sh
pct exec <id> -- bash -c 'cd /root/kiwi && git pull && ./install.sh'
```

### On another Linux computer

On a Linux computer with systemd, install Kiwi as usual ([Linux](install.md#linux)), with the
Tailscale name the bundle was made for and its owner, then import the bundle:

```sh
sudo tailscale up --hostname kiwi
./install.sh --owner you@example.com
./migrate.sh import ~/kiwi-migration-<date>.tar
```

The import checks the bundle, stops Kiwi, puts the Mac's data directory in place of the new
one (which it keeps aside as `/var/lib/kiwi.before-import-<time>`), makes it Kiwi's, and
runs `kiwi migrate import` as Kiwi's user. That command moves the paths of signed apps to
`/var/lib/kiwi`, stores the identity and profiles as uploads (skipping expired profiles and
the App Store and Ad Hoc ones, which aren't for the development certificate) and saves the
Mac builder's pairing. Then Kiwi starts again. If any of that fails, the import puts the
previous data directory back and starts Kiwi as it was. An import over a Kiwi that already
has devices or signed apps needs `--replace`.

## 3. Your phones

Open the new address on each phone, as above. That's all: Kiwi knows the devices and the
apps on them, and **Settings › Mac builder** shows the Mac online.

## Afterwards

Once the new Kiwi has run for a while, delete the bundle everywhere and remove the old Kiwi
from the Mac, which also removes its `tailscale serve` mapping. The Mac builder stays:

```sh
./uninstall.sh            # keeps ~/Library/Application Support/Kiwi
./uninstall.sh --purge    # deletes it too (only once you're sure you won't go back)
```

## Going back

Until the new Kiwi changes anything you care about, you can go back to the Mac. Stop the
new Kiwi (`pct stop <id>`), then on the Mac:

```sh
launchctl enable gui/$(id -u)/local.kiwi
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/local.kiwi.plist
./uninstall.sh --builder   # optional: the Mac builder
```

Kiwi on the Mac starts with its data as it was at the export.

## A trial run

`./migrate.sh export --trial` makes a bundle without stopping Kiwi on the Mac and without the
Mac builder, to try the move first. Kiwi on the Mac keeps running, so both send
notifications and refresh sources until you remove one; delete the trial container before
the real move, so the real one can take the name.

## What the bundle holds

| In the bundle | From the Mac |
|---|---|
| `manifest.json` | the bundle format, the Mac's data directory, the new name and address, the owner |
| `data/` | Kiwi's data directory: a copy of `kiwi.db` made with SQLite's backup, `vapid.json`, `art.key`, `art/`, `icons/`, `downloads/`, `builds/`, `android-app/` and `automation/` (Kiwi Builds' history, artifacts, jobs and Android key) |
| `signing/` | the identity as `identity.p12` with its password, and the profiles from Xcode's folders |
| `mac-builder.json` | the Mac builder's pairing: its name and the SHA-256 of its token (the token itself stays on the Mac) |

Left out: Kiwi's program (`app*`), the `kiwi.db.bak-*` backups, temporary files, and what
belongs to the Mac: the Kiwi Builds toolchain, the decryption enrollment (the Mac builder
keeps using both) and runner logs.
