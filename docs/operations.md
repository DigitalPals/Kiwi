# Operations

Where Kiwi lives on the Mac, how to configure it, and how to look after it.

## What runs where

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

### The data directory

| File | |
|---|---|
| `kiwi.db` | SQLite: sources, devices, people, builds, installs, push subscriptions, settings |
| `kiwi.db.bak-<UTC time>` | Copies `install.sh` takes before each install (the newest 5 are kept) |
| `vapid.json` | The Web Push key pair (0600). If it's lost, every device has to turn notifications on again. An unreadable one is moved to `vapid.json.corrupt-<time>` and replaced |
| `appstoreconnect.json` | The App Store Connect key, when you added one (0600) |
| `profiles/` | Development profiles Kiwi made through App Store Connect |
| `downloads/`, `builds/`, `icons/` | Downloaded IPAs, signed builds and icons; caches that Kiwi cleans up |
| `automation/` | Kiwi Builds' runner, recipes, jobs and artifacts (see [Kiwi Builds](kiwi-builds.md)) |
| `work/` | Temporary files, emptied at every start |
| `kiwi.lock` | Held by the running Kiwi; a second instance on the same data stops |

## Configuration

`install.sh` writes Kiwi's options into the LaunchAgent. Change them by running
`./install.sh` again with other options (see [Install](install.md#5-install-kiwi)). The
server's own flags, also settable through environment variables:

| Flag | Environment | Default |
|---|---|---|
| `--listen` | `KIWI_LISTEN` | `127.0.0.1:8742` |
| `--data-dir` | `KIWI_DATA_DIR` | `~/Library/Application Support/Kiwi` |
| `--web-dir` | `KIWI_WEB_DIR` | `<program's directory>/web` |
| `--public-url` | `KIWI_PUBLIC_URL` | `https://` + the Mac's Tailscale DNS name; requests must use this host or the listen address |
| `--tailscale-socket` | `KIWI_TAILSCALE_SOCKET` | `/var/run/tailscaled.socket` |
| `--tailscale-bin` | `KIWI_TAILSCALE_BIN` | `/opt/homebrew/bin/tailscale` (the installer passes the one it found) |
| `--owner` | `KIWI_OWNER` | The Tailscale user who owns the Mac |
| `--allow-simulators` | `KIWI_ALLOW_SIMULATORS` | Off; treats simulators as linkable devices, for testing |

`RUST_LOG` sets the log level (`info` in the LaunchAgent). Everything else is in the app
under Settings.

## Day to day

```sh
tail -f ~/Library/Logs/Kiwi/kiwi.log                  # the log
launchctl print gui/$(id -u)/local.kiwi | head -30    # state, pid, last exit
launchctl kickstart -k gui/$(id -u)/local.kiwi        # restart
curl -s http://127.0.0.1:8742/api/health              # {"ok":true,"pid":…,"version":"…"}
tailscale serve status                                # / -> http://127.0.0.1:8742
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

## Backups

Back up `kiwi.db`, `vapid.json` and, if you use it, `appstoreconnect.json`. Use
`sqlite3 kiwi.db ".backup kiwi.db.bak"` for a consistent copy of the database while Kiwi
runs. The `kiwi.db.bak-*` copies the installer keeps are on the same disk, so they don't
count as a backup. Everything else in the data directory can be rebuilt.

## Uninstall

```sh
./uninstall.sh            # stop and remove the LaunchAgent, the serve mapping, the log rotation rule and the program
./uninstall.sh --purge    # also delete the data directory and logs
```

Without `--purge`, the data directory and logs stay, so installing again picks up where
you left off. The Kiwi Builds toolchain in `~/.kiwi-builder-tools` is never removed;
delete it yourself if you used Kiwi Builds.

## Working from another machine

To develop on another computer and deploy to the Mac over SSH, put `KIWI_MAC=you@your-mac`
in `deploy/deploy.env` (not committed) and run `deploy/deploy.sh`. It copies the checkout
to `~/kiwi-build` on the Mac, runs `./install.sh` there and checks Kiwi through Tailscale.
`./uninstall.sh` works from there too. Both need SSH key login, your desktop session on the
Mac, and passwordless `sudo` on the Mac (there's no terminal to type a password into). See
[Development](development.md).
