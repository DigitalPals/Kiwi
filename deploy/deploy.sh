#!/usr/bin/env bash
# Deploy this checkout to a Mac over SSH, for working on Kiwi from another machine:
#
#   KIWI_MAC=you@your-mac deploy/deploy.sh [install.sh options]
#
# KIWI_MAC can also be set in deploy/deploy.env (not committed), e.g. KIWI_MAC=you@10.0.0.5.
#
#  1. rsync the checkout to ~/kiwi-build on the Mac (without target/, node_modules/, .git/
#     and web/dist/, so the Mac keeps its own build caches);
#  2. run ./install.sh there (build, install, restart, roll back on failure, tailscale serve;
#     options such as --no-build are passed on);
#  3. check /api/health through Tailscale from here.
#
# Needs SSH key login, a desktop session of that user on the Mac (Kiwi runs in it) and
# passwordless sudo on the Mac for the log rotation rule and tailscale serve.
set -euo pipefail

BUILD_DIR=kiwi-build # relative to the Mac user's home
SSH=(ssh -o BatchMode=yes -o ConnectTimeout=10)

say() { printf '==> %s\n' "$*"; }
die() {
	printf 'deploy: %s\n' "$*" >&2
	exit 1
}

main() {
	local repo log url host ip health
	case "${1:-}" in
	-h | --help)
		sed -n '2,15p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
		return 0
		;;
	esac
	repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
	[ -f "$repo/install.sh" ] || die "$repo is not the Kiwi repository"
	# shellcheck source=/dev/null
	if [ -z "${KIWI_MAC:-}" ] && [ -f "$repo/deploy/deploy.env" ]; then . "$repo/deploy/deploy.env"; fi
	[ -n "${KIWI_MAC:-}" ] || die "set KIWI_MAC=user@mac (or put it in deploy/deploy.env)"

	say "Syncing $repo to $KIWI_MAC:~/$BUILD_DIR"
	rsync -a --delete -e "${SSH[*]}" \
		--exclude=/target/ --exclude=node_modules/ --exclude=/.git --exclude=/web/dist/ \
		--exclude=.DS_Store --exclude=__pycache__/ --exclude=/deploy/deploy.env \
		"$repo/" "$KIWI_MAC:$BUILD_DIR/"
	"${SSH[@]}" "$KIWI_MAC" "touch $BUILD_DIR/.kiwi-deploy-copy"

	log=$(mktemp -t kiwi-deploy.XXXXXX)
	# Expanded now: the trap runs after main returns, when the local $log is gone (set -u).
	# shellcheck disable=SC2064
	trap "rm -f -- '$log'" EXIT
	"${SSH[@]}" "$KIWI_MAC" /bin/bash "$BUILD_DIR/install.sh" "$@" </dev/null | tee "$log"
	url=$(sed -n 's|^==> Kiwi .* is running at \(https://[^/ ]*\)/ .*|\1|p' "$log" | tail -n 1)
	[ -n "$url" ] || die "install.sh did not report Kiwi's address"
	host=${url#https://}

	# This machine may not resolve *.ts.net names (no MagicDNS): then pin the Mac's address.
	say "Checking $url/api/health through Tailscale"
	# shellcheck disable=SC2016 # expanded by the Mac's shell
	ip=$("${SSH[@]}" "$KIWI_MAC" 'PATH=/opt/homebrew/bin:/usr/local/bin:/Applications/Tailscale.app/Contents/MacOS:$PATH; tailscale ip -4 2>/dev/null || Tailscale ip -4' | head -n 1)
	for _ in $(seq 1 30); do
		if health=$(curl -fsS --max-time 5 "$url/api/health" 2>/dev/null) ||
			{ [ -n "$ip" ] && health=$(curl -fsS --max-time 5 --resolve "$host:443:$ip" "$url/api/health" 2>/dev/null); }; then
			say "Kiwi $(jq -r .version <<<"$health") is live at $url/ (tailnet only)"
			return 0
		fi
		sleep 2
	done
	die "$url/api/health is not reachable through Tailscale from $(hostname)"
}

main "$@"
