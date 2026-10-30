#!/bin/bash
# Remove Kiwi from this Mac (or, from another machine, from the Mac in KIWI_MAC over SSH):
#
#   ./uninstall.sh           remove Kiwi and keep its data and logs
#   ./uninstall.sh --purge   also delete the data directory and logs
#
# Stops and removes the LaunchAgent local.kiwi, the `tailscale serve` mapping of / (only
# when it points at Kiwi; other paths stay), /etc/newsyslog.d/kiwi.conf, the installed app
# ("~/Library/Application Support/Kiwi/app", app.prev, app.new, app.old, app.bad) and a
# copy that deploy/deploy.sh synced to ~/kiwi-build. Data ("~/Library/Application
# Support/Kiwi": kiwi.db and its kiwi.db.bak-* backups, vapid.json, downloads, builds,
# icons, automation) and logs (~/Library/Logs/Kiwi) are kept unless --purge is given; the Kiwi Builds toolchain (~/.kiwi-builder-tools) is never
# touched. KIWI_MAC can be set in deploy/deploy.env.
set -euo pipefail

LABEL=local.kiwi
PORT=8742
BUILD_DIR=kiwi-build # deploy/deploy.sh's copy, relative to the Mac user's home
NEWSYSLOG_CONF=/etc/newsyslog.d/kiwi.conf

say() { printf '==> %s\n' "$*"; }
die() {
	printf 'uninstall: %s\n' "$*" >&2
	exit 1
}

as_root() {
	if [ -t 0 ]; then sudo "$@"; else sudo -n "$@"; fi
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

main() {
	local mode=--keep-data mac
	case "${1:-}" in
	"") ;;
	--purge) mode=--purge ;;
	*)
		echo "Usage: ./uninstall.sh [--purge]" >&2
		return 2
		;;
	esac
	if [ "$(uname -s)" = Darwin ]; then
		on_mac "$mode"
		return
	fi
	local env
	env="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/deploy/deploy.env"
	# shellcheck source=/dev/null
	if [ -z "${KIWI_MAC:-}" ] && [ -f "$env" ]; then . "$env"; fi
	mac=${KIWI_MAC:-}
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
else
	main "$@"
fi
