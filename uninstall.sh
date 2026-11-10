#!/bin/bash
# Remove Kiwi from this Mac or Linux computer (or, from another machine, from the Mac in
# KIWI_MAC over SSH):
#
#   ./uninstall.sh           remove Kiwi and keep its data and logs
#   ./uninstall.sh --purge   also delete the data directory and logs
#   ./uninstall.sh --local   on Linux: remove Kiwi from this computer even when KIWI_MAC is set
#
# Stops and removes the LaunchAgent local.kiwi, the `tailscale serve` mapping of / (only
# when it points at Kiwi; other paths stay), /etc/newsyslog.d/kiwi.conf, the installed app
# ("~/Library/Application Support/Kiwi/app", app.prev, app.new, app.old, app.bad) and a
# copy that deploy/deploy.sh synced to ~/kiwi-build. Data ("~/Library/Application
# Support/Kiwi": kiwi.db and its kiwi.db.bak-* backups, vapid.json, downloads, builds,
# icons, automation) and logs (~/Library/Logs/Kiwi) are kept unless --purge is given; the Kiwi Builds toolchain (~/.kiwi-builder-tools) is never
# touched. KIWI_MAC can be set in deploy/deploy.env.
#
# On Linux (as root, through sudo) it stops and removes kiwi.service, the `tailscale serve`
# mapping of / (only when it points at Kiwi) and the program in /opt/kiwi. The data in
# /var/lib/kiwi and the user "kiwi" that owns it stay unless --purge is given (which removes
# that user only when it is the system account install.sh makes). Back up the data first:
# it holds Kiwi Builds' Android signing key (docs/operations.md, "Backups").
set -euo pipefail

LABEL=local.kiwi
PORT=8742
BUILD_DIR=kiwi-build # deploy/deploy.sh's copy, relative to the Mac user's home
NEWSYSLOG_CONF=/etc/newsyslog.d/kiwi.conf
# Linux (see install.sh)
UNIT=/etc/systemd/system/kiwi.service
APPS=/opt/kiwi
LINUX_DATA=/var/lib/kiwi
SERVICE_USER=kiwi

say() { printf '==> %s\n' "$*"; }
die() {
	printf 'uninstall: %s\n' "$*" >&2
	exit 1
}

as_root() {
	if [ "$(id -u)" = 0 ]; then
		"$@"
	elif [ -t 0 ]; then sudo "$@"; else sudo -n "$@"; fi
}

# launchctl for the gui/<uid> domain; see install.sh.
lctl() {
	launchctl "$@" 2>/dev/null && return 0
	if sudo -n launchctl asuser "$(id -u)" sudo -n -u "$(id -un)" launchctl "$@" 2>/dev/null; then
		say "(launchctl $1 needed sudo launchctl asuser)"
		return 0
	fi
	say "(launchctl $1: retrying as root)"
	sudo -n launchctl "$@"
}

find_tailscale() {
	local bin
	for bin in "$(command -v tailscale 2>/dev/null || true)" /opt/homebrew/bin/tailscale \
		/usr/local/bin/tailscale /Applications/Tailscale.app/Contents/MacOS/Tailscale; do
		if [ -n "$bin" ] && [ -x "$bin" ]; then
			printf '%s\n' "$bin"
			return 0
		fi
	done
	return 1
}

# Removes `/` from `tailscale serve` when it points at Kiwi.
remove_serve() {
	local tailscale status host key want="http://127.0.0.1:$PORT" cur after others_before others_after
	if ! tailscale=$(find_tailscale); then
		say "Tailscale is not installed; no serve mapping to remove"
		return 0
	fi
	host=$("$tailscale" status --json 2>/dev/null | jq -r '.Self.DNSName // empty' || true)
	host=${host%.}
	status=$("$tailscale" serve status --json 2>/dev/null || true)
	[ -n "$status" ] || status='{}'
	if [ -z "$host" ]; then
		say "Tailscale does not answer; check 'tailscale serve status' yourself"
		return 0
	fi
	key="$host:443"
	cur=$(jq -r --arg k "$key" '.Web[$k].Handlers["/"].Proxy // empty' <<<"$status")
	if [ "$cur" = "$want" ]; then
		say "tailscale serve: removing / -> $want"
		"$tailscale" serve --yes --https=443 --set-path=/ off >/dev/null 2>&1 ||
			as_root "$tailscale" serve --yes --https=443 --set-path=/ off >/dev/null
		after=$("$tailscale" serve status --json)
		[ -n "$after" ] || after='{}'
		others_before=$(jq -cS --arg k "$key" '.Web[$k].Handlers // {} | del(.["/"])' <<<"$status")
		others_after=$(jq -cS --arg k "$key" '.Web[$k].Handlers // {}' <<<"$after")
		if [ "$others_before" != "$others_after" ]; then
			die "other tailscale serve handlers changed: $others_before -> $others_after (restore them with tailscale serve --bg --set-path=...)"
		fi
	elif [ -n "$cur" ]; then
		say "tailscale serve: / -> $cur is not Kiwi; left as is"
	fi
}

on_mac() {
	local purge=0
	case "${1:-}" in
	--keep-data) ;;
	--purge) purge=1 ;;
	*) die "usage: uninstall.sh --on-mac --keep-data|--purge" ;;
	esac
	export PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
	local data="$HOME/Library/Application Support/Kiwi"
	local logs="$HOME/Library/Logs/Kiwi"
	local plist="$HOME/Library/LaunchAgents/$LABEL.plist"
	local service
	service="gui/$(id -u)/$LABEL"
	[ "$(uname -s)" = Darwin ] || die "must run on macOS"

	if launchctl print "$service" >/dev/null 2>&1; then
		say "Stopping $LABEL"
		lctl bootout "$service" || true
		for _ in $(seq 1 30); do
			launchctl print "$service" >/dev/null 2>&1 || break
			sleep 1
		done
		if launchctl print "$service" >/dev/null 2>&1; then
			die "$service is still loaded 30 s after bootout"
		fi
	fi
	rm -f "$plist"

	remove_serve

	if [ -e "$NEWSYSLOG_CONF" ]; then
		say "Removing $NEWSYSLOG_CONF"
		as_root rm -f "$NEWSYSLOG_CONF"
	fi

	say "Removing the installed app"
	rm -rf "$data/app" "$data/app.new" "$data/app.prev" "$data/app.old" "$data/app.bad"
	rm -f "$logs/kiwi.pid"
	# Only a copy deploy/deploy.sh made, never a clone of yours.
	if [ -f "$HOME/$BUILD_DIR/.kiwi-deploy-copy" ]; then
		say "Removing ~/$BUILD_DIR (deploy/deploy.sh's copy)"
		rm -rf "$HOME/${BUILD_DIR:?}"
	fi

	if [ "$purge" = 1 ]; then
		say "Purging $data and $logs"
		rm -rf "$data" "$logs"
	else
		say "Kept data in $data and logs in $logs (use --purge to delete them)"
	fi
}

# Linux: whether the account $SERVICE_USER is the one install.sh makes: a system account
# with the home $LINUX_DATA and no login shell. Another account of that name is not Kiwi's.
kiwi_service_user() {
	local entry uid home shell uid_min
	entry=$(getent passwd "$SERVICE_USER") || return 1
	IFS=: read -r _ _ uid _ _ home shell <<<"$entry"
	uid_min=$(awk '$1 == "UID_MIN" { print $2; exit }' /etc/login.defs 2>/dev/null || true)
	case "$uid_min" in "" | *[!0-9]*) uid_min=1000 ;; esac
	[ "$uid" != 0 ] && [ "$uid" -lt "$uid_min" ] && [ "$home" = "$LINUX_DATA" ] || return 1
	case "$shell" in
	*/nologin | */false) return 0 ;;
	esac
	return 1
}

# Linux, as root: kiwi.service, the serve mapping and /opt/kiwi; with --purge also
# /var/lib/kiwi and the kiwi user (when install.sh made it).
on_linux() {
	local purge=0 port
	case "${1:-}" in
	--keep-data) ;;
	--purge) purge=1 ;;
	*) die "usage: uninstall.sh --on-linux --keep-data|--purge" ;;
	esac
	[ "$(uname -s)" = Linux ] || die "must run on Linux"
	[ "$(id -u)" = 0 ] || die "must run as root"

	# The port install.sh chose, for recognising Kiwi's serve mapping.
	port=$(sed -n 's/.*--listen 127\.0\.0\.1:\([0-9][0-9]*\).*/\1/p' "$UNIT" 2>/dev/null | head -n 1)
	PORT=${port:-$PORT}
	if [ -f "$UNIT" ]; then
		say "Stopping and removing kiwi.service"
		systemctl disable --now kiwi.service || true
		rm -f "$UNIT"
		systemctl daemon-reload
	fi

	remove_serve

	say "Removing the installed app"
	rm -rf "$APPS/app" "$APPS/app.new" "$APPS/app.prev" "$APPS/app.old" "$APPS/app.bad"
	rmdir "$APPS" 2>/dev/null || true

	if [ "$purge" = 1 ]; then
		say "Purging $LINUX_DATA"
		rm -rf "$LINUX_DATA"
		if kiwi_service_user; then
			say "Removing the user $SERVICE_USER"
			userdel "$SERVICE_USER" || true
		elif id -u "$SERVICE_USER" >/dev/null 2>&1; then
			say "Kept the user $SERVICE_USER: it is not the system account install.sh makes (home $LINUX_DATA, no login shell)"
		fi
	else
		say "Kept data in $LINUX_DATA, owned by the user $SERVICE_USER (use --purge to delete both); logs stay in the journal"
	fi
}

main() {
	local mode=--keep-data mac here=0
	while [ $# -gt 0 ]; do
		case "$1" in
		--purge) mode=--purge ;;
		--local) here=1 ;;
		*)
			echo "Usage: ./uninstall.sh [--purge] [--local]" >&2
			return 2
			;;
		esac
		shift
	done
	if [ "$(uname -s)" = Darwin ]; then
		on_mac "$mode"
		return
	fi
	local env
	env="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/deploy/deploy.env"
	# shellcheck source=/dev/null
	if [ "$here" = 0 ] && [ -z "${KIWI_MAC:-}" ] && [ -f "$env" ]; then . "$env"; fi
	mac=${KIWI_MAC:-}
	if [ "$(uname -s)" = Linux ] && { [ "$here" = 1 ] || [ -z "$mac" ]; }; then
		if [ ! -e "$UNIT" ] && [ ! -d "$APPS" ] && [ ! -d "$LINUX_DATA" ]; then
			die "Kiwi is not installed on this computer. To remove it from a Mac over SSH, set KIWI_MAC=user@mac (or put it in deploy/deploy.env)"
		fi
		as_root "$BASH" "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/uninstall.sh" --on-linux "$mode"
		return
	fi
	[ -n "$mac" ] || die "not on a Mac: set KIWI_MAC=user@mac (or put it in deploy/deploy.env) to uninstall over SSH"
	# Copy this script to the Mac and run it there (stdin stays free for the commands).
	# shellcheck disable=SC2016 # expanded by the remote shell
	ssh -o BatchMode=yes -o ConnectTimeout=10 "$mac" \
		'f=$(mktemp -t kiwi-uninstall) || exit 1; cat >"$f"; /bin/bash "$f" --on-mac '"$mode"' </dev/null; rc=$?; rm -f "$f"; exit $rc' \
		<"${BASH_SOURCE[0]}"
}

if [ "${1:-}" = --on-mac ]; then
	shift
	on_mac "$@"
elif [ "${1:-}" = --on-linux ]; then
	shift
	on_linux "$@"
else
	main "$@"
fi
