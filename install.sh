#!/bin/bash
# Install or update Kiwi on this Mac. Run it from a clone of the repository, in Terminal,
# as the user whose keychain holds your Apple Development certificate:
#
#   ./install.sh           check this Mac, build Kiwi, install or update it and start it
#   ./install.sh --check   only check this Mac and show what would be installed
#
# Options:
#   --no-build         install the last build instead of compiling
#   --public-url URL   the address iPhones open [default: https://<this Mac's Tailscale name>]
#   --owner LOGIN      the owner's Tailscale login [default: the user who owns this Mac in
#                      Tailscale; required when the Mac is tagged]
#   --port PORT        loopback port Kiwi listens on [default: 8742]
#
# What it does:
#  1. checks macOS, your login session, Xcode's command line tools, Rust, Node.js, Tailscale
#     (connected, with HTTPS certificates) and the signing certificate;
#  2. builds the server (cargo build --release) and the web app (npm ci && npm run build);
#  3. stages both in "~/Library/Application Support/Kiwi/app.new", backs up kiwi.db with
#     sqlite3 ".backup" (the newest 5 copies are kept) and swaps the new build in; the
#     running one becomes app.prev/;
#  4. installs the LaunchAgent ~/Library/LaunchAgents/local.kiwi.plist (Kiwi runs in your
#     login session so codesign can use your keychain) and the log rotation rule
#     /etc/newsyslog.d/kiwi.conf (sudo);
#  5. (re)starts Kiwi and waits until the new process answers /api/health on loopback and
#     still does 10 s later. If it does not, it rolls back to the previous build by itself
#     (the database backup is never restored automatically);
#  6. publishes Kiwi on your tailnet with `tailscale serve` (`/` -> 127.0.0.1:PORT, HTTPS,
#     tailnet only, never Funnel), leaving other serve paths alone.
#
# Safe to run again; ./uninstall.sh removes Kiwi. Compatible with macOS's /bin/bash 3.2.
set -euo pipefail

LABEL=local.kiwi
PORT=8742
NEWSYSLOG_CONF=/etc/newsyslog.d/kiwi.conf
DB_BACKUPS=5 # kiwi.db.bak-* copies kept in the data directory
TAILSCALE_SOCKET=/var/run/tailscaled.socket
REPO=$(cd "$(dirname "$0")" && pwd)
DATA="$HOME/Library/Application Support/Kiwi"
LOGS="$HOME/Library/Logs/Kiwi"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
SERVICE="gui/$(id -u)/$LABEL"
# Rust, Homebrew and Tailscale are often missing from the PATH of SSH sessions.
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

CHECK=0
BUILD=1
PUBLIC_URL=
OWNER=
PROBLEMS=0
TAILSCALE=
TS_HOST=
TMP_DIR=
BACKUP=

say() { printf '==> %s\n' "$*"; }
good() { printf '  \342\234\223 %s\n' "$*"; }
note() { printf '  - %s\n' "$*"; }
problem() {
	printf '  \342\234\227 %s\n' "$*"
	PROBLEMS=$((PROBLEMS + 1))
}
die() {
	printf 'install: %s\n' "$*" >&2
	exit 1
}

usage() { sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'; }

# sudo that may ask for a password in Terminal, but never blocks a script without one.
as_root() {
	if [ -t 0 ]; then sudo "$@"; else sudo -n "$@"; fi
}

# launchctl for the gui/<uid> domain. An SSH session may be refused, so retry in the GUI
# session's bootstrap namespace, then as root.
lctl() {
	launchctl "$@" 2>/dev/null && return 0
	if sudo -n launchctl asuser "$(id -u)" sudo -n -u "$(id -un)" launchctl "$@" 2>/dev/null; then
		say "(launchctl $1 needed sudo launchctl asuser)"
		return 0
	fi
	say "(launchctl $1: retrying as root)"
	sudo -n launchctl "$@"
}

# ---------------------------------------------------------------------------------------
# Checks

find_tailscale() {
	local bin
	for bin in "$(command -v tailscale 2>/dev/null || true)" /opt/homebrew/bin/tailscale \
		/usr/local/bin/tailscale /Applications/Tailscale.app/Contents/MacOS/Tailscale; do
		if [ -n "$bin" ] && [ -x "$bin" ]; then
			TAILSCALE=$bin
			return 0
		fi
	done
	return 1
}

# Major version of `node --version` ("v22.12.0" -> 22).
node_major() {
	local v
	v=$(node --version 2>/dev/null) || return 1
	v=${v#v}
	printf '%s\n' "${v%%.*}"
}

check_system() {
	say "Checking this Mac"
	[ "$(uname -s)" = Darwin ] || die "Kiwi runs on macOS; for another machine see deploy/deploy.sh"
	[ "$(id -u)" != 0 ] || die "run this as yourself, not as root (Kiwi uses your login keychain)"
	good "macOS $(sw_vers -productVersion) on $(uname -m), user $(id -un)"
	if launchctl print "gui/$(id -u)" >/dev/null 2>&1; then
		good "Logged in to the desktop (Kiwi runs in this login session)"
	else
		problem "No desktop session for $(id -un): log in to this Mac's desktop first. Kiwi runs in your login session so it can sign with your keychain."
	fi
	if xcrun --find codesign >/dev/null 2>&1; then
		good "Xcode command line tools"
	else
		problem "Xcode command line tools are missing: install Xcode from the App Store, or run: xcode-select --install"
	fi
	if xcrun --find devicectl >/dev/null 2>&1; then
		good "Xcode (devicectl)"
	else
		note "Optional: full Xcode adds devicectl (reads the iOS version and apps of a paired iPhone) and is needed for Kiwi Builds"
	fi
	local identities profiles=0 dir
	identities=$(security find-identity -v -p codesigning 2>/dev/null | grep -c '"Apple Development:' || true)
	if [ "$identities" -gt 0 ]; then
		good "Apple Development certificate in your keychain"
	else
		note "No Apple Development certificate yet: sign in to Xcode with your Apple Developer account (Xcode › Settings › Accounts) and create one. Kiwi installs anyway but can't sign apps until then."
	fi
	for dir in "$HOME/Library/Developer/Xcode/UserData/Provisioning Profiles" "$HOME/Library/MobileDevice/Provisioning Profiles"; do
		if [ -d "$dir" ]; then
			profiles=$((profiles + $(find "$dir" -name '*.mobileprovision' | wc -l)))
		fi
	done
	if [ "$profiles" -gt 0 ]; then
		good "$profiles provisioning profile(s)"
	else
		note "No provisioning profiles yet: build any app for your iPhone once in Xcode (see docs/install.md)"
	fi
	for tool in jq sqlite3 curl; do
		if ! command -v "$tool" >/dev/null; then
			problem "$tool is missing (brew install $tool)"
		fi
	done
	if [ "$BUILD" = 1 ]; then
		if command -v cargo >/dev/null; then
			good "Rust ($(cargo --version))"
		else
			problem "Rust is missing: install it from https://rustup.rs"
		fi
		local major
		if major=$(node_major) && [ "$major" -ge 20 ]; then
			good "Node.js $(node --version)"
		else
			problem "Node.js 20 or newer is missing (brew install node)"
		fi
	fi
}

# Sets TAILSCALE, TS_HOST and PUBLIC_URL.
check_tailscale() {
	say "Checking Tailscale"
	if ! find_tailscale; then
		problem "Tailscale is missing: install it from https://tailscale.com/download/mac (or brew install tailscale) and sign in"
		return
	fi
	local status state tagged certs serve key cur
	if ! status=$("$TAILSCALE" status --json 2>/dev/null) || [ -z "$status" ]; then
		problem "Tailscale ($TAILSCALE) does not answer: open Tailscale and sign in"
		return
	fi
	state=$(jq -r '.BackendState // empty' <<<"$status")
	if [ "$state" != Running ]; then
		problem "Tailscale is not connected (state: ${state:-unknown}): open Tailscale and sign in"
		return
	fi
	# tailscale serve answers on this name; --public-url may name another address for it.
	TS_HOST=$(jq -r '.Self.DNSName // empty' <<<"$status")
	TS_HOST=${TS_HOST%.}
	if [ -z "$TS_HOST" ]; then
		problem "This Mac has no Tailscale DNS name: turn on MagicDNS (https://login.tailscale.com/admin/dns)"
		return
	fi
	PUBLIC_URL=${PUBLIC_URL:-https://$TS_HOST}
	good "Connected as $TS_HOST ($TAILSCALE)"
	certs=$(jq -r '.CertDomains // [] | length' <<<"$status")
	if [ "$certs" -gt 0 ]; then
		good "HTTPS certificates are on"
	else
		problem "HTTPS certificates are off for your tailnet: turn on HTTPS under https://login.tailscale.com/admin/dns"
	fi
	tagged=$(jq -r '.Self.Tags // [] | length' <<<"$status")
	if [ "$tagged" -gt 0 ] && [ -z "$OWNER" ]; then
		problem "This Mac is tagged in Tailscale, so it has no owner: run again with --owner <your Tailscale login>"
	fi
	if [ -S "$TAILSCALE_SOCKET" ]; then
		good "Kiwi asks tailscaled through $TAILSCALE_SOCKET"
	fi
	serve=$("$TAILSCALE" serve status --json 2>/dev/null || true)
	[ -n "$serve" ] || serve='{}'
	key="$TS_HOST:443"
	if [ "$(jq -r --arg k "$key" '.AllowFunnel[$k] // false' <<<"$serve")" = true ]; then
		problem "Funnel is on for $key, which would put Kiwi on the internet: turn it off (tailscale funnel status)"
	fi
	cur=$(jq -r --arg k "$key" '.Web[$k].Handlers["/"].Proxy // .Web[$k].Handlers["/"].Path // empty' <<<"$serve")
	if [ -n "$cur" ] && [ "$cur" != "http://127.0.0.1:$PORT" ]; then
		problem "tailscale serve already maps / of $key to $cur; remove that first or pick it with --public-url"
	fi
}

check_port() {
	local health pid
	if health=$(curl -fsS --max-time 2 "http://127.0.0.1:$PORT/api/health" 2>/dev/null); then
		good "Kiwi $(jq -r '.version // "?"' <<<"$health") is running on port $PORT; this updates it"
	elif pid=$(lsof -nP -t -iTCP:"$PORT" -sTCP:LISTEN 2>/dev/null | head -n 1) && [ -n "$pid" ]; then
		problem "Port $PORT is used by another program (pid $pid): stop it or choose --port"
	fi
}

check_sudo() {
	if [ -t 0 ] || sudo -n true 2>/dev/null; then
		return
	fi
	problem "Without a terminal, sudo must work without a password (log rotation and tailscale serve need it); run ./install.sh in Terminal instead"
}

# ---------------------------------------------------------------------------------------
# Installing

xml_escape() { printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'; }
sed_escape() { printf '%s' "$1" | sed -e 's/[\\|&]/\\&/g'; }

# Renders a template ($1): @HOME@, @PORT@, @PUBLIC_URL@, @TAILSCALE_SOCKET@,
# @TAILSCALE_BIN@, @USER@ and @GROUP@; the @OWNER_ARGS@ line becomes --owner LOGIN or goes.
render() {
	local owner_args=
	if [ -n "$OWNER" ]; then
		owner_args="		<string>--owner</string>\\
		<string>$(sed_escape "$(xml_escape "$OWNER")")</string>"
	fi
	sed -e "s|@HOME@|$(sed_escape "$HOME")|g" -e "s|@PORT@|$PORT|g" \
		-e "s|@PUBLIC_URL@|$(sed_escape "$(xml_escape "$PUBLIC_URL")")|g" \
		-e "s|@TAILSCALE_SOCKET@|$TAILSCALE_SOCKET|g" \
		-e "s|@TAILSCALE_BIN@|$(sed_escape "$(xml_escape "$TAILSCALE")")|g" \
		-e "s|@USER@|$(id -un)|g" -e "s|@GROUP@|$(id -gn)|g" "$1" |
		if [ -n "$owner_args" ]; then
			sed -e "s|^.*@OWNER_ARGS@.*$|$owner_args|"
		else
			sed -e '/@OWNER_ARGS@/d'
		fi
}

service_pid() {
	launchctl print "$SERVICE" 2>/dev/null | awk -F' = ' '$1 == "\tpid" { print $2; exit }'
}

# Prints /api/health when it answers. With a version $2, the answer must also come from
# pid $1 and report that version (a rolled-back older build may not report its pid).
health_of() {
	local health
	health=$(curl -fsS --max-time 2 "http://127.0.0.1:$PORT/api/health" 2>/dev/null) || return 1
	if [ -n "$2" ]; then
		[ "$(jq -r '.pid // empty' <<<"$health")" = "$1" ] || return 1
		[ "$(jq -r '.version // empty' <<<"$health")" = "$2" ] || return 1
	fi
	printf '%s\n' "$health"
}

# Waits up to 90 s for a Kiwi process other than $1 that answers /api/health on loopback
# (as itself, with version $2 when given) and is still the one answering 10 s later.
wait_healthy() {
	local old_pid=$1 version=$2 pid health
	for _ in $(seq 1 90); do
		pid=$(service_pid)
		if [ -n "$pid" ] && [ "$pid" != "$old_pid" ] && health=$(health_of "$pid" "$version"); then
			say "Kiwi answers (pid $pid); checking that it stays up"
			sleep 10
			if [ "$(service_pid)" = "$pid" ] && health=$(health_of "$pid" "$version"); then
				say "Kiwi is running (pid $pid): $health"
				return 0
			fi
			say "Kiwi (pid $pid) did not stay up"
		fi
		sleep 1
	done
	launchctl print "$SERVICE" 2>&1 | sed -n '1,40p' >&2 || true
	tail -n 40 "$LOGS/kiwi.log" >&2 || true
	say "Kiwi did not become healthy on 127.0.0.1:$PORT (agent state and log above)"
	return 1
}

# (Re)starts the agent: bootstrap when it is not loaded, bootout + bootstrap with $1 =
# reload (plist changed), else kickstart -k. Fails instead of exiting.
restart_agent() {
	local uid
	uid=$(id -u)
	if ! launchctl print "$SERVICE" >/dev/null 2>&1; then
		say "Loading $LABEL into gui/$uid"
		lctl enable "$SERVICE" || return 1
		lctl bootstrap "gui/$uid" "$PLIST" || return 1
	elif [ "$1" = reload ]; then
		say "Reloading $LABEL (its LaunchAgent changed)"
		lctl bootout "$SERVICE" || true
		for _ in $(seq 1 30); do
			launchctl print "$SERVICE" >/dev/null 2>&1 || break
			sleep 1
		done
		if launchctl print "$SERVICE" >/dev/null 2>&1; then
			say "$SERVICE is still loaded 30 s after bootout"
			return 1
		fi
		lctl bootstrap "gui/$uid" "$PLIST" || return 1
	else
		say "Restarting $LABEL"
		lctl kickstart -k "$SERVICE" || return 1
	fi
}

# Online copy of kiwi.db (sqlite3 .backup) into the data directory; keeps the newest
# $DB_BACKUPS. Sets BACKUP.
backup_db() {
	local db="$DATA/kiwi.db" dest schema backups=() f i
	[ -f "$db" ] || return 0
	dest="$DATA/kiwi.db.bak-$(date -u +%Y%m%dT%H%M%SZ)"
	sqlite3 -cmd ".timeout 10000" "$db" ".backup '$dest'" || die "could not back up $db"
	chmod 600 "$dest"
	# The copy is in WAL mode like kiwi.db, and macOS's sqlite3 keeps -wal/-shm files after
	# opening it. Switch it to a rollback journal so the backup is one self-contained file.
	schema=$(sqlite3 "$dest" 'PRAGMA journal_mode=DELETE; PRAGMA user_version;' | tail -n 1)
	rm -f "$dest-wal" "$dest-shm"
	BACKUP=$dest
	say "Backed up kiwi.db (schema $schema) to $dest"
	# Backups only (names end in the UTC time's Z), oldest first; their -wal/-shm go with them.
	for f in "$DATA"/kiwi.db.bak-*Z; do
		if [ -f "$f" ]; then backups+=("$f"); fi
	done
	i=0
	while [ $((${#backups[@]} - i)) -gt "$DB_BACKUPS" ]; do
		rm -f "${backups[$i]}" "${backups[$i]}-wal" "${backups[$i]}-shm"
		i=$((i + 1))
	done
}

# Puts app.new/ in place. The running build becomes app.prev/ and the one before it waits in
# app.old/ until the new build is healthy. If app.old/ is left over, an earlier install did
# not finish, so app/ was never confirmed healthy: it is dropped and app.prev/ stays the
# rollback build.
swap_in() {
	if [ -d "$DATA/app.old" ]; then
		say "An earlier install did not finish; keeping app.prev/ as the rollback build"
		rm -rf "$DATA/app"
	else
		if [ -d "$DATA/app.prev" ]; then mv "$DATA/app.prev" "$DATA/app.old"; fi
		if [ -d "$DATA/app" ]; then mv "$DATA/app" "$DATA/app.prev"; fi
	fi
	mv "$DATA/app.new" "$DATA/app"
}

# Restores app.prev/ and the previous plist ($2), restarts and exits with the reason $1.
# The database is not restored (see docs/operations.md, "Roll back").
rollback() {
	local reason=$1 previous_plist=$2 mode=kickstart failed_pid backup_note
	backup_note="Database backup: ${BACKUP:-none (no database yet)}; it was not restored"
	if [ ! -d "$DATA/app.prev" ]; then
		die "$reason. There is no previous build to roll back to. $backup_note. See $LOGS/kiwi.log."
	fi
	say "Install failed ($reason); rolling back to the previous build"
	failed_pid=$(service_pid)
	rm -rf "$DATA/app.bad"
	mv "$DATA/app" "$DATA/app.bad"
	mv "$DATA/app.prev" "$DATA/app"
	if [ -d "$DATA/app.old" ]; then mv "$DATA/app.old" "$DATA/app.prev"; fi
	if [ -f "$previous_plist" ] && ! cmp -s "$previous_plist" "$PLIST"; then
		install -m 644 "$previous_plist" "$PLIST"
		mode=reload
	fi
	if restart_agent "$mode" && wait_healthy "$failed_pid" ""; then
		die "$reason. Rolled back: the previous build is running again and the failed one is in app.bad/. $backup_note."
	fi
	die "$reason. The rollback to the previous build did not become healthy either (the failed build is in app.bad/). $backup_note. If the log says the database schema is newer than this Kiwi, restore the backup (docs/operations.md, \"Roll back\")."
}

# `tailscale serve`, as you first (the Tailscale app), then as root (Homebrew's tailscaled).
serve() {
	"$TAILSCALE" serve "$@" >/dev/null 2>&1 || as_root "$TAILSCALE" serve "$@" >/dev/null
}

ensure_serve() {
	local key="$TS_HOST:443" want="http://127.0.0.1:$PORT" before after cur others_before others_after
	before=$("$TAILSCALE" serve status --json)
	[ -n "$before" ] || before='{}'
	cur=$(jq -r --arg k "$key" '.Web[$k].Handlers["/"].Proxy // .Web[$k].Handlers["/"].Path // empty' <<<"$before")
	if [ "$cur" = "$want" ]; then
		say "tailscale serve: / -> $want (already set)"
		return 0
	elif [ -n "$cur" ]; then
		die "tailscale serve already maps / of $key to $cur; refusing to replace it"
	fi
	say "tailscale serve: mapping https://$TS_HOST/ -> $want (tailnet only)"
	serve --bg --yes --https=443 --set-path=/ "$want"
	after=$("$TAILSCALE" serve status --json)
	cur=$(jq -r --arg k "$key" '.Web[$k].Handlers["/"].Proxy // empty' <<<"$after")
	[ "$cur" = "$want" ] || die "tailscale serve did not take the / mapping (got '$cur')"
	others_before=$(jq -cS --arg k "$key" 'del(.Web[$k].Handlers["/"]) | .Web' <<<"$before")
	others_after=$(jq -cS --arg k "$key" 'del(.Web[$k].Handlers["/"]) | .Web' <<<"$after")
	[ "$others_before" = "$others_after" ] || die "other tailscale serve handlers changed: $others_before -> $others_after"
}

build() {
	say "Building the server (cargo build --release)"
	(cd "$REPO" && cargo build --release --locked -p kiwi-server)
	say "Building the web app (npm ci && npm run build)"
	(cd "$REPO/web" && npm ci --no-audit --no-fund && npm run build)
}

install_kiwi() {
	local mode=kickstart old_pid version
	[ -x "$REPO/target/release/kiwi" ] || die "$REPO/target/release/kiwi is missing; run without --no-build"
	[ -f "$REPO/web/dist/index.html" ] || die "$REPO/web/dist is missing; run without --no-build"

	say "Installing into $DATA/app"
	mkdir -p "$DATA" "$LOGS" "$HOME/Library/LaunchAgents"
	chmod 700 "$DATA" "$LOGS"
	rm -rf "$DATA/app.new"
	mkdir "$DATA/app.new"
	cp "$REPO/target/release/kiwi" "$DATA/app.new/kiwi"
	cp -R "$REPO/web/dist" "$DATA/app.new/web"
	version=$("$DATA/app.new/kiwi" --version) || die "the new binary does not run"
	version=${version##* } # "kiwi 0.2.0" -> "0.2.0"
	render "$REPO/deploy/local.kiwi.plist.in" >"$TMP_DIR/plist"
	plutil -lint -s "$TMP_DIR/plist" || die "the rendered LaunchAgent is invalid"

	backup_db
	swap_in
	if [ -f "$PLIST" ]; then cp "$PLIST" "$TMP_DIR/plist.prev"; fi
	if ! cmp -s "$TMP_DIR/plist" "$PLIST"; then
		install -m 644 "$TMP_DIR/plist" "$PLIST"
		mode=reload
		say "Installed $PLIST"
	fi

	render "$REPO/deploy/kiwi.newsyslog.conf.in" >"$TMP_DIR/newsyslog"
	if ! cmp -s "$TMP_DIR/newsyslog" "$NEWSYSLOG_CONF"; then
		as_root install -m 644 -o root -g wheel "$TMP_DIR/newsyslog" "$NEWSYSLOG_CONF"
		say "Installed $NEWSYSLOG_CONF (log rotation)"
	fi

	old_pid=$(service_pid)
	if ! restart_agent "$mode"; then
		rollback "launchctl could not (re)start $LABEL" "$TMP_DIR/plist.prev"
	fi
	if ! wait_healthy "$old_pid" "$version"; then
		rollback "Kiwi $version did not become healthy" "$TMP_DIR/plist.prev"
	fi
	rm -rf "$DATA/app.old"
	ensure_serve
	say "Kiwi $version is running at $PUBLIC_URL/ (tailnet only)"
}

next_steps() {
	cat <<EOF

Next:
  1. On your iPhone, install Tailscale and sign in to the same tailnet.
  2. Open $PUBLIC_URL/ in Safari, tap Share › Add to Home Screen, and open Kiwi from there.
  3. In Kiwi, check Settings › Signing, then add an AltStore source under Sources.

Logs: $LOGS/kiwi.log
Docs: $REPO/docs/install.md · Update: git pull && ./install.sh · Remove: ./uninstall.sh
EOF
}

main() {
	while [ $# -gt 0 ]; do
		case "$1" in
		--check) CHECK=1 ;;
		--no-build) BUILD=0 ;;
		--public-url)
			[ $# -ge 2 ] || die "--public-url needs a URL"
			PUBLIC_URL=${2%/}
			shift
			;;
		--owner)
			[ $# -ge 2 ] || die "--owner needs a Tailscale login"
			OWNER=$2
			shift
			;;
		--port)
			case "${2:-}" in
			"" | *[!0-9]*) die "--port needs a port number" ;;
			esac
			if [ "$2" -lt 1 ] || [ "$2" -gt 65535 ]; then die "--port needs a port number"; fi
			PORT=$2
			shift
			;;
		-h | --help)
			usage
			return 0
			;;
		*)
			usage >&2
			return 2
			;;
		esac
		shift
	done
	case "$PUBLIC_URL" in
	"" | https://*) ;;
	*) die "--public-url must start with https:// (iPhones only install apps over HTTPS)" ;;
	esac
	[ -f "$REPO/crates/kiwi-server/Cargo.toml" ] || die "$REPO is not the Kiwi repository"

	check_system
	check_tailscale
	check_port
	if [ "$CHECK" = 0 ]; then check_sudo; fi
	TMP_DIR=$(mktemp -d -t kiwi-install)
	trap 'rm -rf "$TMP_DIR"' EXIT

	if [ "$CHECK" = 1 ]; then
		say "Plan"
		note "Kiwi: $DATA/app, listening on 127.0.0.1:$PORT"
		note "Address: ${PUBLIC_URL:-unknown} (tailscale serve, tailnet only)"
		note "LaunchAgent: $PLIST"
		if [ -n "$TAILSCALE" ]; then
			render "$REPO/deploy/local.kiwi.plist.in" >"$TMP_DIR/plist"
			if [ ! -f "$PLIST" ]; then
				note "The LaunchAgent will be created"
			elif cmp -s "$TMP_DIR/plist" "$PLIST"; then
				note "The installed LaunchAgent stays as it is"
			else
				note "The installed LaunchAgent will change:"
				diff -u "$PLIST" "$TMP_DIR/plist" | sed 's/^/      /' || true
			fi
		fi
		if [ "$PROBLEMS" -gt 0 ]; then
			say "$PROBLEMS problem(s) to fix before installing"
			return 1
		fi
		say "Ready to install: run ./install.sh"
		return 0
	fi
	[ "$PROBLEMS" = 0 ] || die "fix the $PROBLEMS problem(s) above, then run ./install.sh again"
	if [ -t 0 ]; then
		say "Some steps need administrator rights (log rotation, tailscale serve)"
		sudo -v || die "sudo is needed"
	fi
	if [ "$BUILD" = 1 ]; then build; fi
	install_kiwi
	next_steps
}

main "$@"
