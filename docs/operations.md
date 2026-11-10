# Operations

Where Kiwi lives on a Mac, on Linux and in Docker, how to configure it, and how to look
after it.

## What runs where

### On a Mac

| What | Where |
|---|---|
| Service | LaunchAgent `local.kiwi` in your login (Aqua) session, `gui/<your uid>`, so `codesign` can use your login keychain. `KeepAlive` restarts it after a crash |
| LaunchAgent | `~/Library/LaunchAgents/local.kiwi.plist`, rendered from `deploy/local.kiwi.plist.in` |
| Program | `~/Library/Application Support/Kiwi/app/kiwi`. The previous build is kept in `app.prev/`; after an automatic rollback, the failed build is in `app.bad/` |
| Web app | `~/Library/Application Support/Kiwi/app/web` (the built `web/dist`) |
| Listens on | `127.0.0.1:8742`, loopback only |
| Address | `https://<your Mac>.<your tailnet>.ts.net/` through `tailscale serve`: `/` → `http://127.0.0.1:8742`, tailnet only, never Funnel. Other serve paths are left alone |
| Tailscale | Kiwi asks tailscaled who each device is and who owns the Mac, through its LocalAPI socket (`/var/run/tailscaled.socket`, Homebrew's tailscaled) or, without one, the `tailscale` CLI (the Tailscale app). The log says which at start |
| Data | `~/Library/Application Support/Kiwi/` (see below) |
| Log | `~/Library/Logs/Kiwi/kiwi.log` (stdout and stderr); `kiwi.pid` next to it |
| Log rotation | `/etc/newsyslog.d/kiwi.conf`: rotated above 10 MB, 5 gzipped copies kept; Kiwi is then restarted with a fresh log |

Signing uses the Apple Development identity in your login keychain and the profiles in
`~/Library/Developer/Xcode/UserData/Provisioning Profiles/` and
`~/Library/MobileDevice/Provisioning Profiles/`; Kiwi only reads them.

### On Linux

| What | Where |
|---|---|
| Service | The system service `kiwi.service` (`/etc/systemd/system/kiwi.service`, rendered from `deploy/kiwi.service.in`), running as the user `kiwi`. Starts at boot; restarted after a crash. Sandboxed: it can write only its data directory |
| Program | `/opt/kiwi/app/kiwi` and `/opt/kiwi/app/web`, owned by root. The previous build is kept in `app.prev/`; after an automatic rollback, the failed build is in `app.bad/` |
| Listens on | `127.0.0.1:8742`, loopback only |
| Address | `https://<this computer>.<your tailnet>.ts.net/` through `tailscale serve`, as on a Mac |
| Tailscale | tailscaled's LocalAPI socket, `/var/run/tailscale/tailscaled.sock` |
| Data | `/var/lib/kiwi/` (owned by `kiwi`, mode 0700) |
| Log | The journal: `journalctl -u kiwi` |

Signing uses an identity and profiles uploaded to Kiwi, kept in the data directory under
`signing/` ([Install](install.md#signing-ios-apps-on-linux)).

### In Docker

| What | Where |
|---|---|
| Containers | `kiwi` (the image from `Dockerfile`, as the user `kiwi`, uid 10001) and `tailscale`, whose network Kiwi shares (`compose.yaml`) |
| Listens on | `127.0.0.1:8742` inside the shared network; nothing is published on the host |
| Address | `https://<TS_HOSTNAME>.<your tailnet>.ts.net/`, from the Tailscale container's `tailscale serve` |
| Tailscale | The Tailscale container's socket, shared through the volume `tailscale-socket` |
| Data | The volume `kiwi-data`, `/data` in the container; the Tailscale machine in `tailscale-state` |
| Log | `docker compose logs kiwi` |

### The data directory

| File | |
|---|---|
| `kiwi.db` | SQLite: sources, devices, builds, installs, push subscriptions, settings |
| `kiwi.db.bak-<UTC time>` | Copies `install.sh` takes before each install (the newest 5 are kept) |
| `vapid.json` | The Web Push key pair (0600). If it's lost, every device has to turn notifications on again. An unreadable one is moved to `vapid.json.corrupt-<time>` and replaced |
| `downloads/`, `builds/`, `icons/` | Downloaded IPAs, signed builds and icons; caches that Kiwi cleans up |
| `automation/` | Kiwi Builds' runner, recipes, jobs and artifacts (see [Kiwi Builds](kiwi-builds.md)) |
| `android-app/` | Kiwi's Android app as this Kiwi signed it, and unsigned builds you place in `unsigned/` (see [Kiwi's Android app](android-app.md)) |
| `work/` | Temporary files, emptied at every start |
| `kiwi.lock` | Held by the running Kiwi; a second instance on the same data stops |

## Configuration

`install.sh` writes Kiwi's options into the LaunchAgent (on Linux, into `kiwi.service`).
Change them by running `./install.sh` again with other options (see
[Install](install.md#5-install-kiwi)). In Docker, set them as environment variables in
`compose.yaml`. The server's own flags, also settable through environment variables:

| Flag | Environment | Default |
|---|---|---|
| `--listen` | `KIWI_LISTEN` | `127.0.0.1:8742` |
| `--data-dir` | `KIWI_DATA_DIR` | `~/Library/Application Support/Kiwi` on a Mac; elsewhere `$XDG_DATA_HOME/kiwi`, that is `~/.local/share/kiwi` (the installer uses `/var/lib/kiwi`, the image `/data`) |
| `--web-dir` | `KIWI_WEB_DIR` | `<program's directory>/web` |
| `--public-url` | `KIWI_PUBLIC_URL` | `https://` + this computer's Tailscale DNS name; requests must use this host or the listen address |
| `--tailscale-socket` | `KIWI_TAILSCALE_SOCKET` | `/var/run/tailscaled.socket` on a Mac, `/var/run/tailscale/tailscaled.sock` on Linux |
| `--tailscale-bin` | `KIWI_TAILSCALE_BIN` | `/opt/homebrew/bin/tailscale` on a Mac, `/usr/bin/tailscale` on Linux (the installer passes the one it found) |
| `--python` | `KIWI_PYTHON` | The Python for Kiwi Builds' runners: `/opt/homebrew/bin/python3.13` on a Mac, `python3` on Linux (Android builds need 3.12 or newer) |
| `--owner` | `KIWI_OWNER` | The Tailscale user who owns this computer |
| `--proxy-uid` | `KIWI_PROXY_UID` | Not on a Mac: `0`, the user tailscaled runs as; only its connections may forward tailnet addresses (see [Install](install.md)) |
| `--trust-local` | `KIWI_TRUST_LOCAL` | Off; takes every user and program on the computer for the owner, as on a Mac (development, or a computer only you use) |
| `--allow-simulators` | `KIWI_ALLOW_SIMULATORS` | Off; treats simulators as linkable devices, for testing |

Without `--public-url`, Kiwi asks Tailscale for this computer's name at start, and keeps
asking in the background while tailscaled is still starting. The App Store country starts as the
region of the Mac's locale, on Linux of `LC_ALL`, `LC_MESSAGES` or `LANG`, until you pick one
in Settings. `RUST_LOG` sets the log level (`info` in the LaunchAgent, the service and the
image). Everything else is in the app under Settings.

## Day to day

On a Mac:

```sh
tail -f ~/Library/Logs/Kiwi/kiwi.log                  # the log
launchctl print gui/$(id -u)/local.kiwi | head -30    # state, pid, last exit
launchctl kickstart -k gui/$(id -u)/local.kiwi        # restart
curl -s http://127.0.0.1:8742/api/health              # {"ok":true,"pid":…,"version":"…"}
tailscale serve status                                # / -> http://127.0.0.1:8742
```

On Linux:

```sh
journalctl -u kiwi -f                                 # the log
systemctl status kiwi                                 # state, pid, last exit
sudo systemctl restart kiwi                           # restart
curl -s http://127.0.0.1:8742/api/health              # {"ok":true,"pid":…,"version":"…"}
tailscale serve status                                # / -> http://127.0.0.1:8742
```

In Docker, from the directory with `compose.yaml`:

```sh
docker compose logs -f kiwi                           # the log
docker compose ps                                     # state and health
docker compose restart kiwi                           # restart
docker compose exec tailscale tailscale serve status  # / -> http://127.0.0.1:8742
```

## Updating

```sh
cd kiwi && git pull && ./install.sh
```

The installer is safe to run again. It builds Kiwi, stages the new build in `app.new/`,
backs up `kiwi.db` with `sqlite3 ".backup"` to `kiwi.db.bak-<UTC time>` and swaps the
build in: the running build becomes `app.prev/`, and the one before it waits in `app.old/`
until the new one is healthy. It reloads the LaunchAgent only when it changed (otherwise
`launchctl kickstart -k`), then waits until a new Kiwi process answers `/api/health` with
the new version, and still does 10 s later.

If launchctl fails or Kiwi doesn't become healthy within 90 s, the installer rolls back by
itself: the failed build moves to `app.bad/`, `app.prev/` and the previous LaunchAgent come
back, Kiwi restarts, and the installer exits with an error naming the database backup. It
never restores the database by itself.

On Linux it does the same with `/opt/kiwi/app` and `kiwi.service`, and takes the backup as
the `kiwi` user. In Docker, `git pull && docker compose up -d --build` builds a new image and
replaces the container; take a backup first (below).

## Roll back

To go back to the previous build by hand:

```sh
cd ~/Library/Application\ Support/Kiwi && rm -rf app.bad && mv app app.bad && mv app.prev app
launchctl kickstart -k gui/$(id -u)/local.kiwi
```

A newer Kiwi may have migrated `kiwi.db` to a schema the older one doesn't know. The older
build then stops with "The database schema (version N) is newer than this Kiwi" in the log.
Only then, restore the backup taken before the upgrade (changes made since are lost):

```sh
cd ~/Library/Application\ Support/Kiwi
launchctl bootout gui/$(id -u)/local.kiwi
ls -1 kiwi.db.bak-*                                  # the installer printed which one it took
sqlite3 kiwi.db.bak-<time> 'PRAGMA user_version'     # the backup's schema
mv kiwi.db kiwi.db.newer; mv kiwi.db-wal kiwi.db.newer-wal 2>/dev/null; rm -f kiwi.db-shm
cp kiwi.db.bak-<time> kiwi.db && chmod 600 kiwi.db
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/local.kiwi.plist
```

On Linux the builds are in `/opt/kiwi` and the data in `/var/lib/kiwi`:

```sh
cd /opt/kiwi && sudo rm -rf app.bad && sudo mv app app.bad && sudo mv app.prev app
sudo systemctl restart kiwi
```

To restore a database backup there, stop Kiwi with `sudo systemctl stop kiwi`, move the
files as above in `/var/lib/kiwi` as root, `sudo chown kiwi:kiwi kiwi.db`, and start it with
`sudo systemctl start kiwi`.

## Backups

Back up these files from the data directory, to another disk:

| File | Why |
|---|---|
| `kiwi.db` | Sources, devices, installs and settings. Use `sqlite3 kiwi.db ".backup kiwi.db.bak"` for a consistent copy while Kiwi runs |
| `vapid.json` | The Web Push key: without it every device has to turn notifications on again |
| `automation/android/keystore.p12` and `automation/android/keystore.pass` | Kiwi's Android signing key and its password: it signs Kiwi Builds' Android apps and [Kiwi's own Android app](android-app.md). **Android only accepts an update signed with the same key** as the installed app: without these two files, every Android app Kiwi installed, Kiwi's app included, has to be removed from each phone before Kiwi can install it again, losing the app's data |
| `signing/` | On Linux and in Docker: the uploaded Apple Development identity (`.p12`) and profiles. You can upload them again from your own copies |

The `kiwi.db.bak-*` copies the installer keeps are on the same disk, so they don't count as
a backup. Everything else in the data directory can be rebuilt. The data directory is
private (on Linux owned by `kiwi`), so copy it as root and keep the copy private too: it
holds signing keys.

In Docker, copy the files out of the running container:

```sh
docker compose exec kiwi python3 -c "import sqlite3; sqlite3.connect('/data/kiwi.db').backup(sqlite3.connect('/data/kiwi.db.bak'))"
docker compose cp kiwi:/data/kiwi.db.bak ./kiwi.db
docker compose cp kiwi:/data/vapid.json ./
docker compose cp kiwi:/data/automation/android ./android   # keystore.p12, keystore.pass
docker compose cp kiwi:/data/signing ./signing              # when you upload an iOS identity
```

`docker compose cp` fails for a path that doesn't exist yet (no Android build, no uploaded
identity); that's fine.

## Uninstall

```sh
./uninstall.sh            # stop and remove the LaunchAgent, the serve mapping, the log rotation rule and the program
./uninstall.sh --purge    # also delete the data directory and logs
```

Without `--purge`, the data directory and logs stay, so installing again picks up where
you left off. The Kiwi Builds toolchain in `~/.kiwi-builder-tools` is never removed;
delete it yourself if you used Kiwi Builds.

On Linux, `./uninstall.sh` removes `kiwi.service`, the serve mapping and `/opt/kiwi`, and
keeps `/var/lib/kiwi` and the user `kiwi`; `--purge` deletes them too. When `KIWI_MAC` is
set (below), `./uninstall.sh` removes Kiwi from that Mac instead; `--local` removes it from
this Linux computer. In Docker, `docker compose down` stops and removes the containers and
keeps the volumes; `docker compose down --volumes` deletes them, Kiwi's data included.

## Working from another machine

To develop on another computer and deploy to the Mac over SSH, put `KIWI_MAC=you@your-mac`
in `deploy/deploy.env` (not committed) and run `deploy/deploy.sh`. It copies the checkout
to `~/kiwi-build` on the Mac, runs `./install.sh` there and checks Kiwi through Tailscale.
`./uninstall.sh` works from there too. Both need SSH key login, your desktop session on the
Mac, and passwordless `sudo` on the Mac (there's no terminal to type a password into). See
[Development](development.md).
