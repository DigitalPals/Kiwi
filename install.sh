#!/bin/bash
# Install or update Kiwi on this Mac or Linux computer. Run it from a clone of the repository:
# on a Mac in Terminal, as the user whose keychain holds your Apple Development certificate;
# on Linux as a user who may use sudo (or as root).
#
#   ./install.sh           check this computer, build Kiwi, install or update it and start it
#   ./install.sh --check   only check this computer and show what would be installed
#
# Options:
#   --no-build         install the last build instead of compiling
#   --public-url URL   the address phones open [default: https://<this computer's Tailscale name>]
#   --owner LOGIN      the owner's Tailscale login [default: the user who owns this computer
#                      in Tailscale; required when it is tagged]
#   --port PORT        loopback port Kiwi listens on [default: 8742]
#   --python PATH      Linux: Python 3.12 or newer for Kiwi Builds' Android apps, installed
#                      outside /home [default: python3 in /usr/local/bin, /usr/bin or /bin]
#
#   ./install.sh --builder --server URL --pair CODE   instead: a Mac builder, which builds
#                      iOS apps for a Kiwi elsewhere (./install.sh --builder --help)
#
# What it does on a Mac:
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
# On Linux it builds as you, then installs as root (sudo): Kiwi runs as the system service
# kiwi.service (deploy/kiwi.service.in) under its own user "kiwi", the program in
# /opt/kiwi/app (app.prev/ and the rest as above), data in /var/lib/kiwi, logs in the journal
# (journalctl -u kiwi). It checks, backs up, swaps, restarts, rolls back and runs
# `tailscale serve` as on a Mac.
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
# Where the installed builds live (app/, app.prev/, ...), the service definition, its log,
# and how messages name this computer and its Tailscale.
APPS=$DATA
SERVICE_FILE=$PLIST
LOG_HINT="$LOGS/kiwi.log"
MACHINE="This Mac"
TS_INSTALL="install it from https://tailscale.com/download/mac (or brew install tailscale) and sign in"
TS_START="open Tailscale and sign in"

LINUX=0
if [ "$(uname -s)" = Linux ]; then
	LINUX=1
	TAILSCALE_SOCKET=/var/run/tailscale/tailscaled.sock
	DATA=/var/lib/kiwi
	APPS=/opt/kiwi
	SERVICE_USER=kiwi
	UNIT=/etc/systemd/system/kiwi.service
	SERVICE_FILE=$UNIT
	LOG_HINT="journalctl -u kiwi"
	MACHINE="This computer"
	TS_INSTALL="install it from https://tailscale.com/download/linux and sign in (sudo tailscale up)"
	TS_START="start it and sign in (sudo systemctl enable --now tailscaled && sudo tailscale up)"
fi

CHECK=0
BUILD=1
PUBLIC_URL=
OWNER=
PROBLEMS=0
TAILSCALE=
TS_HOST=
TMP_DIR=
BACKUP=
PYTHON=
PYTHON_GIVEN=0 # --python named it
PYTHON_OK=0    # it runs and is new enough

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

usage() { sed -n '2,19p' "$0" | sed 's/^# \{0,1\}//'; }

# sudo that may ask for a password in Terminal, but never blocks a script without one. As
# root (Linux only) it runs the command itself.
as_root() {
	if [ "$(id -u)" = 0 ]; then
		"$@"
	elif [ -t 0 ]; then sudo "$@"; else sudo -n "$@"; fi
}

# Runs a command on the data directory as its owner: you on a Mac, the kiwi user on Linux.
as_data() {
	if [ "$LINUX" = 1 ]; then runuser -u "$SERVICE_USER" -- "$@"; else "$@"; fi
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
# Whether node can run the locked Vite (20.19 or newer in 20.x, else 22.12 or newer).
node_supported() {
	local v major minor
	v=$(node --version 2>/dev/null) || return 1
	v=${v#v}
	major=${v%%.*}
	minor=${v#*.}
	minor=${minor%%.*}
	case "$major$minor" in "" | *[!0-9]*) return 1 ;; esac
	[ "$major" -gt 22 ] || { [ "$major" = 22 ] && [ "$minor" -ge 12 ]; } ||
		{ [ "$major" = 20 ] && [ "$minor" -ge 19 ]; }
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
		if node_supported; then
			good "Node.js $(node --version)"
		else
			problem "Node.js 20.19 or 22.12 (or newer) is missing (brew install node)"
		fi
	fi
}

# Linux: why an existing account named $SERVICE_USER is not one install.sh made (a system
# account with the home $DATA and no login shell), as a message; nothing when it is, or when
# there is none yet. Kiwi would run as that account and its data and keys would be its.
foreign_service_user() {
	local entry uid home shell uid_min why=
	entry=$(getent passwd "$SERVICE_USER") || return 0
	IFS=: read -r _ _ uid _ _ home shell <<<"$entry"
	uid_min=$(awk '$1 == "UID_MIN" { print $2; exit }' /etc/login.defs 2>/dev/null || true)
	case "$uid_min" in "" | *[!0-9]*) uid_min=1000 ;; esac
	if [ "$uid" = 0 ] || [ "$uid" -ge "$uid_min" ]; then
		why="it is not a system account (uid $uid)"
	elif [ "$home" != "$DATA" ]; then
		why="its home is $home, not $DATA"
	else
		case "$shell" in
		*/nologin | */false) ;;
		*) why="it has the login shell $shell" ;;
		esac
	fi
	[ -n "$why" ] || return 0
	printf '%s\n' "The user $SERVICE_USER already exists, but $why, so install.sh did not make it for Kiwi (Kiwi runs as $SERVICE_USER, which owns its data and signing keys). If another program uses that account, Kiwi can't share it. If it is unused, remove it (sudo userdel $SERVICE_USER); then run ./install.sh again, which makes its own."
}

# Linux: directories kiwi.service can't see (ProtectHome=yes, PrivateTmp=yes).
hidden_from_service() {
	case "$1" in
	/home | /home/* | /root | /root/* | /run/user | /run/user/* | /tmp | /tmp/* | /var/tmp | /var/tmp/*) return 0 ;;
	esac
	return 1
}

# Linux: the Python kiwi.service runs Kiwi Builds' Android runner with. Without --python,
# python3 from the system's directories, never from your PATH: pyenv, uv, mise, conda,
# Linuxbrew and virtualenvs keep theirs in your home, which the service can't see. Sets
# PYTHON and PYTHON_OK.
check_python() {
	local dir version
	if [ "$PYTHON_GIVEN" = 0 ]; then
		PYTHON=python3
		for dir in /usr/local/bin /usr/bin /bin; do
			if [ -x "$dir/python3" ] && ! hidden_from_service "$(readlink -f "$dir/python3")"; then
				PYTHON=$dir/python3
				break
			fi
		done
		if [ "$PYTHON" = python3 ]; then
			note "Optional: Python 3.12 or newer as python3 in /usr/local/bin or /usr/bin (or named with --python) lets Kiwi Builds build Android apps"
			return
		fi
	else
		case "$PYTHON" in
		/*) ;;
		*/*)
			problem "--python needs an absolute path, not $PYTHON"
			return
			;;
		*)
			for dir in /usr/local/bin /usr/bin /bin; do
				if [ -x "$dir/$PYTHON" ]; then
					PYTHON=$dir/$PYTHON
					break
				fi
			done
			case "$PYTHON" in
			/*) ;;
			*)
				problem "--python $PYTHON: there is no $PYTHON in /usr/local/bin, /usr/bin or /bin; name its full path"
				return
				;;
			esac
			;;
		esac
		if [ ! -f "$PYTHON" ] || [ ! -x "$PYTHON" ]; then
			problem "--python $PYTHON is not a program"
			return
		fi
		if hidden_from_service "$PYTHON" || hidden_from_service "$(readlink -f "$PYTHON")"; then
			problem "--python $PYTHON: kiwi.service can't use a Python in /home, /root, /run/user or /tmp (it runs as the user $SERVICE_USER, without access to them). Install Python 3.12 or newer system-wide (your package manager, or a build in /usr/local) and name that one"
			return
		fi
	fi
	if version=$("$PYTHON" -c 'import sys; print("%d.%d" % sys.version_info[:2]); sys.exit(sys.version_info < (3, 12))' 2>/dev/null); then
		PYTHON_OK=1
		good "Python $version ($PYTHON) for Kiwi Builds' Android apps"
	elif [ "$PYTHON_GIVEN" = 1 ]; then
		problem "--python $PYTHON is Python ${version:-of an unknown version}; Kiwi Builds needs 3.12 or newer"
	else
		note "Optional: $PYTHON is Python ${version:-of an unknown version}; Kiwi Builds needs 3.12 or newer for Android apps (install one system-wide and name it with --python)"
	fi
}

# Linux, as root: runs $PYTHON the way kiwi.service will (as $SERVICE_USER, without /home,
# /root, /run/user and the real /tmp) and prints its version, or what went wrong.
service_python() {
	local out rc=0
	out=$(systemd-run --quiet --wait --pipe --collect \
		-p User="$SERVICE_USER" -p Group="$SERVICE_USER" -p ProtectHome=yes -p PrivateTmp=yes \
		-p ProtectSystem=strict -p PrivateDevices=yes -p NoNewPrivileges=yes -p UMask=0077 \
		"$PYTHON" -c 'import sys; print("%d.%d" % sys.version_info[:2]); sys.exit(sys.version_info < (3, 12))' \
		</dev/null 2>&1) || rc=$?
	if [ "$rc" = 0 ]; then
		printf '%s\n' "$out"
		return 0
	fi
	case "$out" in
	[0-9]*.[0-9]*) out="it is Python $out there; Kiwi Builds needs 3.12 or newer" ;;
	"") out="it does not start there (exit status $rc): it, or something it needs, is in a directory the service can't see or the user $SERVICE_USER can't read" ;;
	esac
	printf '%s\n' "$out"
	return 1
}

# Linux: the distribution, systemd, tools, the service's user, Python for Kiwi Builds and
# the build tools. Sets PYTHON.
check_linux() {
	say "Checking this computer"
	local distro tool major
	distro=$(sed -n 's/^PRETTY_NAME=//p' /etc/os-release 2>/dev/null | tr -d '"')
	good "${distro:-Linux} on $(uname -m), user $(id -un)"
	if [ -d /run/systemd/system ] && command -v systemctl >/dev/null; then
		good "systemd (Kiwi runs as the system service kiwi.service)"
	else
		problem "systemd is not running here, and install.sh sets Kiwi up as a systemd service. Use Docker instead (docs/install.md)"
	fi
	for tool in jq sqlite3 curl; do
		if ! command -v "$tool" >/dev/null; then
			problem "$tool is missing (install it with your package manager, e.g. sudo apt install $tool)"
		fi
	done
	if [ "$(id -u)" = 0 ]; then
		for tool in useradd runuser; do
			command -v "$tool" >/dev/null || problem "$tool is missing"
		done
	elif ! command -v sudo >/dev/null; then
		problem "sudo is missing: run ./install.sh as root"
	fi
	local foreign
	foreign=$(foreign_service_user)
	if [ -n "$foreign" ]; then problem "$foreign"; fi
	check_python
	note "Kiwi Builds builds iOS apps on a Mac only; Android apps build here"
	if [ "$BUILD" = 1 ]; then
		if command -v cargo >/dev/null; then
			good "Rust ($(cargo --version))"
		else
			problem "Rust is missing: install it from https://rustup.rs"
		fi
		if node_supported; then
			good "Node.js $(node --version)"
		else
			problem "Node.js 20.19 or 22.12 (or newer) is missing (https://nodejs.org, or your package manager)"
		fi
	fi
}

# Sets TAILSCALE, TS_HOST and PUBLIC_URL.
check_tailscale() {
	say "Checking Tailscale"
	if ! find_tailscale; then
		problem "Tailscale is missing: $TS_INSTALL"
		return
	fi
	local status state tagged certs serve key cur
	if ! status=$("$TAILSCALE" status --json 2>/dev/null) || [ -z "$status" ]; then
		problem "Tailscale ($TAILSCALE) does not answer: $TS_START"
		return
	fi
	state=$(jq -r '.BackendState // empty' <<<"$status")
	if [ "$state" != Running ]; then
		problem "Tailscale is not connected (state: ${state:-unknown}): $TS_START"
		return
	fi
	# tailscale serve answers on this name; --public-url may name another address for it.
	TS_HOST=$(jq -r '.Self.DNSName // empty' <<<"$status")
	TS_HOST=${TS_HOST%.}
	if [ -z "$TS_HOST" ]; then
		problem "$MACHINE has no Tailscale DNS name: turn on MagicDNS (https://login.tailscale.com/admin/dns)"
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
		problem "$MACHINE is tagged in Tailscale, so it has no owner: run again with --owner <your Tailscale login>"
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
	elif [ "$LINUX" = 1 ] && ss -Hltn "sport = :$PORT" 2>/dev/null | grep -q .; then
		problem "Port $PORT is used by another program (ss -ltnp shows which): stop it or choose --port"
	fi
}

check_sudo() {
	if [ "$(id -u)" = 0 ] || [ -t 0 ] || sudo -n true 2>/dev/null; then
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

# A value for the unit's ExecStart, in double quotes: systemd unescapes \\ and \", and
# reads % (specifiers) and $ (variables) unless doubled.
unit_quote() {
	printf '"%s"' "$(printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/%/%%/g' -e 's/\$/$$/g')"
}

# Renders deploy/kiwi.service.in: @APP@, @DATA@, @PORT@, @PUBLIC_URL@, @TAILSCALE_SOCKET@,
# @TAILSCALE_BIN@ and @PYTHON@; " @OWNER_ARGS@" becomes " --owner LOGIN" or goes.
render_unit() {
	local owner_args=
	if [ -n "$OWNER" ]; then owner_args=" --owner $(unit_quote "$OWNER")"; fi
	sed -e "s|@APP@|$(sed_escape "$APPS/app")|g" -e "s|@DATA@|$(sed_escape "$DATA")|g" \
		-e "s|@PORT@|$PORT|g" \
		-e "s|@PUBLIC_URL@|$(sed_escape "$(unit_quote "$PUBLIC_URL")")|g" \
		-e "s|@TAILSCALE_SOCKET@|$(sed_escape "$TAILSCALE_SOCKET")|g" \
		-e "s|@TAILSCALE_BIN@|$(sed_escape "$(unit_quote "$TAILSCALE")")|g" \
		-e "s|@PYTHON@|$(sed_escape "$(unit_quote "$PYTHON")")|g" \
		-e "s| @OWNER_ARGS@|$(sed_escape "$owner_args")|" "$REPO/deploy/kiwi.service.in"
}

service_pid() {
	if [ "$LINUX" = 1 ]; then
		systemctl show -p MainPID --value kiwi.service 2>/dev/null | grep -v '^0$' || true
		return
	fi
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
	if [ "$LINUX" = 1 ]; then
		systemctl status --no-pager --lines=0 kiwi.service >&2 || true
		journalctl -u kiwi.service -n 40 --no-pager >&2 || true
		say "Kiwi did not become healthy on 127.0.0.1:$PORT (service state and log above)"
		return 1
	fi
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

# (Re)starts Kiwi: the LaunchAgent on a Mac; on Linux kiwi.service, after reloading systemd
# when $1 = reload (the unit changed). Fails instead of exiting.
restart_service() {
	if [ "$LINUX" = 0 ]; then
		restart_agent "$1"
		return
	fi
	if [ "$1" = reload ]; then
		systemctl daemon-reload || return 1
	fi
	systemctl enable --quiet kiwi.service || return 1
	say "Restarting kiwi.service"
	systemctl restart kiwi.service
}

# Online copy of kiwi.db (sqlite3 .backup) into the data directory; keeps the newest
# $DB_BACKUPS. Sets BACKUP.
backup_db() {
	local db="$DATA/kiwi.db" dest schema backups=() f i
	[ -f "$db" ] || return 0
	dest="$DATA/kiwi.db.bak-$(date -u +%Y%m%dT%H%M%SZ)"
	as_data sqlite3 -cmd ".timeout 10000" "$db" ".backup '$dest'" || die "could not back up $db"
	chmod 600 "$dest"
	# The copy is in WAL mode like kiwi.db, and macOS's sqlite3 keeps -wal/-shm files after
	# opening it. Switch it to a rollback journal so the backup is one self-contained file.
	schema=$(as_data sqlite3 "$dest" 'PRAGMA journal_mode=DELETE; PRAGMA user_version;' | tail -n 1)
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
	if [ -d "$APPS/app.old" ]; then
		say "An earlier install did not finish; keeping app.prev/ as the rollback build"
		rm -rf "$APPS/app"
	else
		if [ -d "$APPS/app.prev" ]; then mv "$APPS/app.prev" "$APPS/app.old"; fi
		if [ -d "$APPS/app" ]; then mv "$APPS/app" "$APPS/app.prev"; fi
	fi
	mv "$APPS/app.new" "$APPS/app"
}

# Restores app.prev/ and the previous plist or unit ($2), restarts and exits with the reason
# $1. The database is not restored (see docs/operations.md, "Roll back").
rollback() {
	local reason=$1 previous_plist=$2 mode=kickstart failed_pid backup_note
	backup_note="Database backup: ${BACKUP:-none (no database yet)}; it was not restored"
	if [ ! -d "$APPS/app.prev" ]; then
		die "$reason. There is no previous build to roll back to. $backup_note. See $LOG_HINT."
	fi
	say "Install failed ($reason); rolling back to the previous build"
	failed_pid=$(service_pid)
	rm -rf "$APPS/app.bad"
	mv "$APPS/app" "$APPS/app.bad"
	mv "$APPS/app.prev" "$APPS/app"
	if [ -d "$APPS/app.old" ]; then mv "$APPS/app.old" "$APPS/app.prev"; fi
	if [ -f "$previous_plist" ] && ! cmp -s "$previous_plist" "$SERVICE_FILE"; then
		install -m 644 "$previous_plist" "$SERVICE_FILE"
		mode=reload
	fi
	if restart_service "$mode" && wait_healthy "$failed_pid" ""; then
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

# Linux, as root: the kiwi user, /opt/kiwi/app, /var/lib/kiwi and kiwi.service.
install_kiwi_linux() {
	local mode=restart old_pid version nologin group=--user-group foreign out
	[ -x "$REPO/target/release/kiwi" ] || die "$REPO/target/release/kiwi is missing; run without --no-build"
	[ -f "$REPO/web/dist/index.html" ] || die "$REPO/web/dist is missing; run without --no-build"

	foreign=$(foreign_service_user)
	[ -z "$foreign" ] || die "$foreign"
	if ! id -u "$SERVICE_USER" >/dev/null 2>&1; then
		nologin=$(command -v nologin || echo /bin/false)
		if getent group "$SERVICE_USER" >/dev/null; then group="--gid=$SERVICE_USER"; fi
		useradd --system "$group" --home-dir "$DATA" --no-create-home --shell "$nologin" \
			"$SERVICE_USER" || die "could not create the user $SERVICE_USER"
		say "Created the system user $SERVICE_USER"
	fi
	install -d -o "$SERVICE_USER" -g "$SERVICE_USER" -m 700 "$DATA"
	install -d -m 755 "$APPS"
	if [ "$PYTHON_OK" = 1 ]; then
		if out=$(service_python); then
			say "kiwi.service can run Python $out ($PYTHON)"
		elif [ "$PYTHON_GIVEN" = 1 ]; then
			die "kiwi.service can't use --python $PYTHON: $out. Name a Python 3.12 or newer installed for the whole system"
		else
			say "Note: kiwi.service can't use $PYTHON ($out), so Kiwi Builds can't build Android apps; name a Python 3.12 or newer it can run with --python"
		fi
	fi

	say "Installing into $APPS/app"
	rm -rf "$APPS/app.new"
	install -d -m 755 "$APPS/app.new"
	install -m 755 "$REPO/target/release/kiwi" "$APPS/app.new/kiwi"
	cp -R "$REPO/web/dist" "$APPS/app.new/web"
	# Owned by root and readable by kiwi: the service can't change its own program.
	chown -R root:root "$APPS/app.new"
	chmod -R u=rwX,go=rX "$APPS/app.new"
	version=$("$APPS/app.new/kiwi" --version) || die "the new binary does not run"
	version=${version##* } # "kiwi 0.2.0" -> "0.2.0"
	render_unit >"$TMP_DIR/unit"

	backup_db
	swap_in
	if [ -f "$UNIT" ]; then cp "$UNIT" "$TMP_DIR/unit.prev"; fi
	if ! cmp -s "$TMP_DIR/unit" "$UNIT"; then
		install -m 644 "$TMP_DIR/unit" "$UNIT"
		mode=reload
		say "Installed $UNIT"
	fi

	old_pid=$(service_pid)
	if ! restart_service "$mode"; then
		rollback "systemctl could not (re)start kiwi.service" "$TMP_DIR/unit.prev"
	fi
	if ! wait_healthy "$old_pid" "$version"; then
		rollback "Kiwi $version did not become healthy" "$TMP_DIR/unit.prev"
	fi
	rm -rf "$APPS/app.old"
	ensure_serve
	say "Kiwi $version is running at $PUBLIC_URL/ (tailnet only)"
}

next_steps_linux() {
	cat <<EOF

Next:
  1. On your phone, install Tailscale and sign in to the same tailnet.
  2. Open $PUBLIC_URL/ in the browser (Safari on an iPhone, then Share › Add to Home Screen).
  3. On an iPhone, upload your signing identity and profiles in Settings › Signing, then add
     an AltStore source under Sources. On Android, see Kiwi Builds under Sources.

Logs: journalctl -u kiwi -f · Data: $DATA
Docs: $REPO/docs/install.md · Update: git pull && ./install.sh · Remove: ./uninstall.sh
EOF
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
	local arg
	for arg in "$@"; do
		if [ "$arg" = --builder ]; then
			builder_main "$@"
			return
		fi
	done
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
		--python)
			[ $# -ge 2 ] || die "--python needs the path of a Python 3.12 or newer"
			[ "$LINUX" = 1 ] || die "--python is for Linux; on a Mac, Kiwi Builds uses Homebrew's python3.13"
			PYTHON=$2
			PYTHON_GIVEN=1
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

	if [ "$LINUX" = 1 ]; then
		check_linux
	else
		check_system
	fi
	check_tailscale
	check_port
	if [ "$CHECK" = 0 ]; then check_sudo; fi
	if [ "$LINUX" = 1 ]; then
		TMP_DIR=$(mktemp -d -t kiwi-install.XXXXXX)
	else
		TMP_DIR=$(mktemp -d -t kiwi-install)
	fi
	trap 'rm -rf "$TMP_DIR"' EXIT

	if [ "$LINUX" = 1 ]; then
		linux_main
		return
	fi

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

# ---------------------------------------------------------------------------------------
# The Mac builder (./install.sh --builder): this Mac builds iOS apps for a Kiwi that runs
# elsewhere (docs/kiwi-builds.md, "Mac builder"). It never touches Kiwi's own LaunchAgent
# (local.kiwi) or data directory; --import-enrollment only reads the latter.

B_DATA="$HOME/Library/Application Support/Kiwi Builder"
B_LOG="$HOME/Library/Logs/kiwi-builder.log"
B_PYTHON=/opt/homebrew/bin/python3.13
SERVER=
PAIR_CODE=
B_NAME=
IMPORT= # a Kiwi data directory to copy the decryption enrollment from

builder_usage() {
	cat <<'EOF'
Install or update Kiwi's Mac builder on this Mac: it builds iOS apps (Kiwi Builds) for a
Kiwi that runs on Linux or in Docker, which signs, publishes and installs them.

  ./install.sh --builder --server URL --pair CODE   pair with Kiwi, build and install
  ./install.sh --builder                            update an installed builder
  ./install.sh --builder --check                    only check this Mac

Options:
  --server URL        Kiwi's address, as in Kiwi's Settings › Mac builder
                      (https://kiwi.example.ts.net); http:// only to this Mac (a tunnel)
  --pair CODE         the one-time code from Settings › Mac builder (10 minutes)
  --name NAME         how Kiwi names this Mac [default: its computer name]
  --import-enrollment [DIR]
                      copy the decryption enrollment (automation/models) from a Kiwi data
                      directory on this Mac [default: ~/Library/Application Support/Kiwi]
  --no-build          install the last build instead of compiling

It installs the program in "~/Library/Application Support/Kiwi Builder/app" and the
LaunchAgent local.kiwi-builder (log: ~/Library/Logs/kiwi-builder.log, rotated by
/etc/newsyslog.d/kiwi-builder.conf). Kiwi's own LaunchAgent and data stay untouched.
./uninstall.sh --builder removes it.
EOF
}

check_builder() {
	say "Checking this Mac (Mac builder)"
	[ "$(uname -s)" = Darwin ] || die "the Mac builder runs on macOS; on this computer, install Kiwi itself (./install.sh)"
	[ "$(id -u)" != 0 ] || die "run this as yourself, not as root (the builder works in your login session)"
	good "macOS $(sw_vers -productVersion) on $(uname -m), user $(id -un)"
	if launchctl print "gui/$(id -u)" >/dev/null 2>&1; then
		good "Logged in to the desktop (the builder runs in this login session)"
	else
		problem "No desktop session for $(id -un): log in to this Mac's desktop first. The builder runs in your login session, where the App Store sign-in and decryption work."
	fi
	if xcrun --sdk iphoneos --show-sdk-path >/dev/null 2>&1; then
		good "Xcode with the iOS SDK"
	else
		problem "Kiwi Builds needs Xcode with the iOS SDK: install Xcode from the App Store, open it once, then: sudo xcode-select -s /Applications/Xcode.app"
	fi
	if [ -x "$B_PYTHON" ]; then
		good "Homebrew's Python 3.13"
	else
		problem "Homebrew's Python 3.13 is missing: brew install python@3.13"
	fi
	if [ "$BUILD" = 1 ] && ! command -v cargo >/dev/null; then
		problem "Rust is missing: install it from https://rustup.rs (or use --no-build with an earlier build)"
	fi
	if ! command -v curl >/dev/null; then
		problem "curl is missing"
	elif [ -n "$SERVER" ]; then
		if curl -fsS --max-time 10 -o /dev/null "$SERVER/api/health"; then
			good "Kiwi answers at $SERVER"
		else
			problem "Kiwi doesn't answer at $SERVER/api/health: check the address, and that this Mac is in Kiwi's tailnet"
		fi
	fi
	if [ -f "$B_DATA/builder.json" ]; then
		good "Paired with Kiwi (builder.json)"
	elif [ -z "$PAIR_CODE" ]; then
		problem "Not paired yet: in Kiwi, open Settings › Mac builder › Pair a Mac and pass its --server and --pair"
	fi
	if [ -f "$B_DATA/automation/runner.py" ] && [ -x "$B_PYTHON" ]; then
		builder_doctor
	else
		note "The build tools are checked once the builder is installed"
	fi
}

# What runner.py doctor says about the build tools. Not problems: the steps after
# installing fix them.
builder_doctor() {
	local line
	"$B_PYTHON" "$B_DATA/automation/runner.py" doctor --data-dir "$B_DATA" 2>/dev/null |
		"$B_PYTHON" -c 'import json, sys
for line in sys.stdin:
    try:
        event = json.loads(line)
    except ValueError:
        continue
    if event.get("event") == "health":
        for check in event.get("checks", []):
            print(("ready " if check.get("ready") else "missing ") + str(check.get("name")))' |
		while IFS= read -r line; do
			case "$line" in
			"ready "*) good "${line#ready }" ;;
			*) note "Not ready yet: ${line#missing } (see the steps after installing, or docs/kiwi-builds.md)" ;;
			esac
		done
}

# Copies automation/models/*.bin (0600) from the Kiwi data directory $IMPORT.
import_enrollment() {
	local source="$IMPORT/automation/models" count=0 model
	[ -d "$source" ] || die "no decryption enrollment in $source; enroll this Mac instead (docs/kiwi-builds.md)"
	mkdir -p "$B_DATA/automation/models"
	chmod 700 "$B_DATA/automation" "$B_DATA/automation/models"
	for model in "$source"/*.bin; do
		[ -f "$model" ] || continue
		install -m 600 "$model" "$B_DATA/automation/models/"
		count=$((count + 1))
	done
	[ "$count" -gt 0 ] || die "no decryption models (*.bin) in $source"
	say "Copied $count decryption model(s) from $source"
	# Share that Kiwi's build tools (and its ipatool) when they live elsewhere than the
	# default ~/.kiwi-builder-tools, rather than setting them up a second time.
	local tools
	if [ ! -e "$B_DATA/automation/tools" ] && [ ! -L "$B_DATA/automation/tools" ] &&
		tools=$(readlink "$IMPORT/automation/tools" 2>/dev/null) && [ -d "$tools" ]; then
		ln -s "$tools" "$B_DATA/automation/tools"
		say "Using the build tools in $tools"
	fi
}

install_builder() {
	local mode=kickstart version pid='' args
	[ -x "$REPO/target/release/kiwi" ] || die "$REPO/target/release/kiwi is missing; run without --no-build"
	say "Installing into $B_DATA/app"
	mkdir -p "$B_DATA" "$HOME/Library/Logs" "$HOME/Library/LaunchAgents"
	chmod 700 "$B_DATA"
	rm -rf "$B_DATA/app.new"
	mkdir "$B_DATA/app.new"
	cp "$REPO/target/release/kiwi" "$B_DATA/app.new/kiwi"
	version=$("$B_DATA/app.new/kiwi" --version) || die "the new binary does not run"
	version=${version##* } # "kiwi 0.2.0" -> "0.2.0"
	if [ -n "$PAIR_CODE" ]; then
		say "Pairing with Kiwi at $SERVER"
		args=(builder pair --data-dir "$B_DATA" --server "$SERVER" --code "$PAIR_CODE")
		if [ -n "$B_NAME" ]; then args+=(--name "$B_NAME"); fi
		if ! "$B_DATA/app.new/kiwi" "${args[@]}"; then
			rm -rf "$B_DATA/app.new"
			die "pairing failed (above). Make a new code in Kiwi's Settings › Mac builder and run this again."
		fi
	fi
	[ -f "$B_DATA/builder.json" ] || die "this Mac isn't paired: pass --server and --pair (Kiwi's Settings › Mac builder)"
	if [ -n "$IMPORT" ]; then import_enrollment; fi
	rm -rf "$B_DATA/app.prev"
	if [ -d "$B_DATA/app" ]; then mv "$B_DATA/app" "$B_DATA/app.prev"; fi
	mv "$B_DATA/app.new" "$B_DATA/app"

	render "$REPO/deploy/local.kiwi-builder.plist.in" >"$TMP_DIR/plist"
	plutil -lint -s "$TMP_DIR/plist" || die "the rendered LaunchAgent is invalid"
	if ! cmp -s "$TMP_DIR/plist" "$PLIST"; then
		install -m 644 "$TMP_DIR/plist" "$PLIST"
		mode=reload
		say "Installed $PLIST"
	fi
	render "$REPO/deploy/kiwi-builder.newsyslog.conf.in" >"$TMP_DIR/newsyslog"
	if ! cmp -s "$TMP_DIR/newsyslog" "$NEWSYSLOG_CONF"; then
		as_root install -m 644 -o root -g wheel "$TMP_DIR/newsyslog" "$NEWSYSLOG_CONF"
		say "Installed $NEWSYSLOG_CONF (log rotation)"
	fi
	restart_agent "$mode" || die "launchctl could not (re)start $LABEL"
	for _ in $(seq 1 15); do
		pid=$(service_pid)
		[ -z "$pid" ] || break
		sleep 1
	done
	sleep 5
	if [ -z "$pid" ] || [ "$(service_pid)" != "$pid" ]; then
		launchctl print "$SERVICE" 2>&1 | sed -n '1,30p' >&2 || true
		tail -n 30 "$B_LOG" >&2 || true
		die "the Mac builder did not stay up (agent state and log above)"
	fi
	say "Kiwi Builder $version is running (pid $pid) for $SERVER"
}

builder_next_steps() {
	cat <<EOF

Next, once, in Terminal on this Mac (docs/kiwi-builds.md, "Mac builder"):

  KIWI_DATA="$B_DATA"
  # 1. The build tools
  $B_PYTHON "\$KIWI_DATA/automation/runner.py" setup --data-dir "\$KIWI_DATA"
  # 2. The App Store sign-in
  "\$KIWI_DATA/automation/tools/bin/ipatool" auth login
  # 3. Decryption (not needed after --import-enrollment)
  $B_PYTHON "\$KIWI_DATA/automation/runner.py" enroll --data-dir "\$KIWI_DATA" --app "/Applications/<App>.app"
  # 4. Check, then use Check Again in Kiwi's Settings › Mac builder
  $B_PYTHON "\$KIWI_DATA/automation/runner.py" doctor --data-dir "\$KIWI_DATA"

Log: $B_LOG
Update: git pull && ./install.sh --builder · Remove: ./uninstall.sh --builder
EOF
}

builder_main() {
	LABEL=local.kiwi-builder
	PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
	SERVICE="gui/$(id -u)/$LABEL"
	NEWSYSLOG_CONF=/etc/newsyslog.d/kiwi-builder.conf
	while [ $# -gt 0 ]; do
		case "$1" in
		--builder) ;;
		--check) CHECK=1 ;;
		--no-build) BUILD=0 ;;
		--server)
			[ $# -ge 2 ] || die "--server needs Kiwi's address"
			SERVER=${2%/}
			shift
			;;
		--pair)
			[ $# -ge 2 ] || die "--pair needs the code from Kiwi's Settings › Mac builder"
			PAIR_CODE=$2
			shift
			;;
		--name)
			[ $# -ge 2 ] || die "--name needs a name"
			B_NAME=$2
			shift
			;;
		--import-enrollment)
			if [ $# -ge 2 ] && [ "${2#--}" = "$2" ]; then
				IMPORT=${2%/}
				shift
			else
				IMPORT="$HOME/Library/Application Support/Kiwi"
			fi
			;;
		-h | --help)
			builder_usage
			return 0
			;;
		*)
			builder_usage >&2
			return 2
			;;
		esac
		shift
	done
	if [ -z "$SERVER" ] && [ -f "$B_DATA/builder.json" ]; then
		SERVER=$(sed -n 's/^ *"server": *"\([^"]*\)".*/\1/p' "$B_DATA/builder.json" | head -n 1)
	fi
	if [ -n "$PAIR_CODE" ] && [ -z "$SERVER" ]; then die "--pair needs --server (Kiwi's address)"; fi
	case "$SERVER" in
	"" | https://* | http://127.0.0.1:* | http://localhost:* | "http://[::1]:"*) ;;
	*) die "--server must be Kiwi's https:// address (http:// only to this Mac), got $SERVER" ;;
	esac
	[ -f "$REPO/crates/kiwi-server/Cargo.toml" ] || die "$REPO is not the Kiwi repository"
	check_builder
	TMP_DIR=$(mktemp -d -t kiwi-builder-install)
	trap 'rm -rf "$TMP_DIR"' EXIT

	if [ "$CHECK" = 1 ]; then
		say "Plan"
		note "Program: $B_DATA/app · LaunchAgent: $PLIST · Log: $B_LOG"
		note "Kiwi: ${SERVER:-unknown}"
		if [ -n "$IMPORT" ]; then note "Decryption enrollment copied from $IMPORT"; fi
		render "$REPO/deploy/local.kiwi-builder.plist.in" >"$TMP_DIR/plist"
		if [ ! -f "$PLIST" ]; then
			note "The LaunchAgent will be created"
		elif cmp -s "$TMP_DIR/plist" "$PLIST"; then
			note "The installed LaunchAgent stays as it is"
		else
			note "The installed LaunchAgent will change:"
			diff -u "$PLIST" "$TMP_DIR/plist" | sed 's/^/      /' || true
		fi
		if [ "$PROBLEMS" -gt 0 ]; then
			say "$PROBLEMS problem(s) to fix before installing"
			return 1
		fi
		say "Ready to install: run it again without --check"
		return 0
	fi
	[ "$PROBLEMS" = 0 ] || die "fix the $PROBLEMS problem(s) above, then run ./install.sh --builder again"
	if [ -t 0 ]; then
		say "Log rotation needs administrator rights"
		sudo -v || die "sudo is needed"
	fi
	if [ "$BUILD" = 1 ]; then
		say "Building the program (cargo build --release)"
		(cd "$REPO" && cargo build --release --locked -p kiwi-server)
	fi
	install_builder
	builder_next_steps
}

# Linux, after the checks: show the plan, or build as you and install as root.
linux_main() {
	if [ "$CHECK" = 1 ]; then
		say "Plan"
		note "Kiwi: $APPS/app as the user $SERVICE_USER, listening on 127.0.0.1:$PORT"
		note "Data: $DATA · Logs: journalctl -u kiwi"
		note "Address: ${PUBLIC_URL:-unknown} (tailscale serve, tailnet only)"
		note "Service: $UNIT"
		if [ -n "$TAILSCALE" ]; then
			render_unit >"$TMP_DIR/unit"
			if [ ! -f "$UNIT" ]; then
				note "The service will be created"
			elif cmp -s "$TMP_DIR/unit" "$UNIT"; then
				note "The installed service stays as it is"
			else
				note "The installed service will change:"
				diff -u "$UNIT" "$TMP_DIR/unit" | sed 's/^/      /' || true
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
	if [ "$BUILD" = 1 ]; then build; fi
	if [ "$(id -u)" != 0 ]; then
		local args=(--no-build --port "$PORT")
		# Without --python, the root run looks in the same system directories.
		if [ "$PYTHON_GIVEN" = 1 ]; then args+=(--python "$PYTHON"); fi
		if [ -n "$PUBLIC_URL" ]; then args+=(--public-url "$PUBLIC_URL"); fi
		if [ -n "$OWNER" ]; then args+=(--owner "$OWNER"); fi
		say "Installing as root (sudo)"
		as_root "$BASH" "$REPO/install.sh" "${args[@]}"
		return
	fi
	install_kiwi_linux
	next_steps_linux
}

main "$@"
