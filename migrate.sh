#!/bin/bash
# Move Kiwi from a Mac to Linux (docs/migrate.md): everything Kiwi keeps comes along, and
# the Mac stays on as the Mac builder for iOS apps.
#
#   ./migrate.sh export [options]   on the Mac, in Terminal: pack Kiwi into one file
#   ./migrate.sh import BUNDLE      on Linux, after ./install.sh: unpack it into Kiwi
#
# export packs Kiwi's data directory (sources, devices, installs, settings, signed apps,
# Kiwi Builds' apps, history and keys, the Web Push key) with a consistent copy of kiwi.db,
# the signing identity from the login keychain (as a .p12; macOS asks for your login
# password to allow that), the provisioning profiles Xcode keeps and a pairing for the Mac
# builder. Then it stops Kiwi on this Mac (its data stays, so you can go back) and installs
# the Mac builder, paired ahead of time with the new Kiwi's address, which it keeps trying
# until the new Kiwi answers.
#
# Export options:
#   --name NAME          the new computer's Tailscale name [default: kiwi]: Kiwi's new
#                        address is https://NAME.<your tailnet>.ts.net
#   --out FILE           the bundle [default: ~/kiwi-migration-<date>.tar]
#   --builder-name NAME  how Kiwi names this Mac as its builder [default: its computer name]
#   --no-builder         don't make this Mac the Mac builder (no iOS apps from Kiwi Builds)
#   --trial              a test copy: Kiwi keeps running here, no Mac builder
#   --p12 FILE           sign with this .p12 instead of exporting the keychain's identity
#                        (its password is asked for, or read from --p12-password-file FILE)
#   --keychain FILE      export the identity from this keychain [default: the login keychain]
#   --identity SHA1      the identity to export, when the keychain has several (security
#                        find-identity -v -p codesigning lists them)
#   --replace-builder    pair this Mac's builder again when it is already paired
#   --no-build           use the last build (target/release/kiwi) for the Mac builder
#
# Import options:
#   --replace            replace a Kiwi that already has data (it is kept aside, not deleted)
#
# The bundle holds your signing keys: keep it private and delete it once the new Kiwi runs.
# Compatible with macOS's /bin/bash 3.2.
set -euo pipefail

FORMAT=1 # manifest.json "format"; crates/kiwi-server/src/migrate.rs reads it
REPO=$(cd "$(dirname "$0")" && pwd)
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

say() { printf '==> %s\n' "$*"; }
good() { printf '  \342\234\223 %s\n' "$*"; }
note() { printf '  - %s\n' "$*"; }
die() {
	printf 'migrate: %s\n' "$*" >&2
	exit 1
}
usage() { sed -n '2,35p' "$0" | sed 's/^# \{0,1\}//'; }

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

# ---------------------------------------------------------------------------------------
# Export (macOS)

DATA="$HOME/Library/Application Support/Kiwi"
B_DATA="$HOME/Library/Application Support/Kiwi Builder"
LABEL=local.kiwi
NAME=kiwi
OUT=
BUILDER=1
BUILDER_NAME=
TRIAL=0
P12=
P12_PASSWORD_FILE=
KEYCHAIN="$HOME/Library/Keychains/login.keychain-db"
IDENTITY=
REPLACE_BUILDER=0
BUILD=1
WORK=
STOPPED=0 # Kiwi on this Mac was stopped by this run
DONE=0    # the bundle is complete

# Undoes a stop when the export fails before the bundle is complete.
export_cleanup() {
	if [ -n "$WORK" ]; then rm -rf "$WORK"; fi
	if [ -n "$OUT" ] && [ "$DONE" = 0 ]; then rm -f "$OUT.partial"; fi
	if [ "$STOPPED" = 1 ] && [ "$DONE" = 0 ]; then
		say "The export failed: starting Kiwi on this Mac again"
		start_mac_kiwi || printf 'migrate: start it yourself: launchctl enable gui/%s/%s && launchctl bootstrap gui/%s %s\n' \
			"$(id -u)" "$LABEL" "$(id -u)" "$HOME/Library/LaunchAgents/$LABEL.plist" >&2
	fi
}

start_mac_kiwi() {
	launchctl enable "gui/$(id -u)/$LABEL" &&
		launchctl bootstrap "gui/$(id -u)" "$HOME/Library/LaunchAgents/$LABEL.plist"
}

mac_kiwi_pid() {
	launchctl print "gui/$(id -u)/$LABEL" 2>/dev/null | awk -F' = ' '$1 == "\tpid" { print $2; exit }'
}

# The top-level entries of the data directory and of automation/ that move (as paths
# relative to the data directory). Left out: Kiwi's program (app*), the database (copied
# with sqlite3 instead) and its backups, temporary files, and what belongs to this Mac:
# the Kiwi Builds toolchain, the decryption enrollment and runner logs. Kiwi writes its
# runner scripts again when it starts.
data_entries() {
	local entry name sub
	for entry in "$DATA"/* "$DATA"/.[!.]*; do
		[ -e "$entry" ] || [ -L "$entry" ] || continue
		name=${entry##*/}
		case "$name" in
		app | app.* | kiwi.db | kiwi.db-* | kiwi.db.* | kiwi.lock | work | profiles | .DS_Store) ;;
		automation)
			for sub in "$DATA"/automation/* "$DATA"/automation/.[!.]*; do
				[ -e "$sub" ] || [ -L "$sub" ] || continue
				name=${sub##*/}
				case "$name" in
				tools | models | __pycache__ | *.log | *.py | .DS_Store) ;;
				*) printf 'automation/%s\n' "$name" ;;
				esac
			done
			;;
		*) printf '%s\n' "$name" ;;
		esac
	done
}

# The identity Kiwi signs with: of the valid code signing identities in the keychain, the
# one of Kiwi's newest signed app, else the only one.
signing_sha1() {
	local db=$1 sha1 list count
	if [ -n "$IDENTITY" ]; then
		printf '%s\n' "$IDENTITY" | tr 'a-f' 'A-F'
		return 0
	fi
	list=$(security find-identity -v -p codesigning "$KEYCHAIN" | sed -n 's/^ *[0-9]*) \([0-9A-Fa-f]\{40\}\) "\(.*\)"$/\1 \2/p')
	sha1=$(sqlite3 "$db" "SELECT upper(identity_sha1) FROM builds WHERE identity_sha1 IS NOT NULL ORDER BY created_at DESC LIMIT 1" 2>/dev/null || true)
	if [ -n "$sha1" ] && printf '%s\n' "$list" | tr 'a-f' 'A-F' | grep -q "^$sha1 "; then
		printf '%s\n' "$sha1"
		return 0
	fi
	count=$(printf '%s\n' "$list" | grep -c . || true)
	if [ "$count" != 1 ]; then
		printf '%s\n' "$list" >&2
		die "found $count valid code signing identities in $KEYCHAIN; name the one Kiwi signs with: --identity SHA1"
	fi
	printf '%s\n' "${list%% *}" | tr 'a-f' 'A-F'
}

# Exports the identity $1 (SHA-1) from the keychain into $2 (a .p12 with the password in
# $3), alone and encrypted as Keychain Access does (3DES), which Kiwi on Linux reads.
export_identity() {
	local sha1=$1 p12=$2 password_file=$3 dir="$WORK/identity" pw out block id cert='' key=''
	mkdir -m 700 "$dir"
	pw=$(/usr/bin/openssl rand -hex 24)
	say "Exporting the signing identity from the keychain"
	note "macOS asks for your login password to allow this (choose Allow, not Always Allow)"
	if ! out=$(security export -k "$KEYCHAIN" -t identities -f pkcs12 -P "$pw" -o "$dir/all.p12" 2>&1); then
		printf '%s\n' "$out" >&2
		die "the keychain did not export the identity: allow it when macOS asks, in Terminal on the Mac's desktop"
	fi
	# Split into one PEM file per certificate and key, with its localKeyID.
	KIWI_PW=$pw /usr/bin/openssl pkcs12 -in "$dir/all.p12" -passin env:KIWI_PW -nodes 2>/dev/null |
		awk -v dir="$dir" '
			/localKeyID:/ { sub(/^.*localKeyID: */, ""); gsub(/ /, ""); id = $0 }
			/^-----BEGIN / { n++; kind = ($0 ~ /PRIVATE KEY/) ? "key" : "cert"; file = dir "/" n "." kind
				print id > (file ".id"); close(file ".id") }
			file != "" { print > file }
			/^-----END / { close(file); file = ""; id = "" }' ||
		die "could not read the exported identity"
	for block in "$dir"/*.cert; do
		[ -f "$block" ] || continue
		if [ "$(/usr/bin/openssl x509 -in "$block" -noout -fingerprint -sha1 | sed 's/.*=//; s/://g' | tr 'a-f' 'A-F')" = "$sha1" ]; then
			cert=$block
			id=$(cat "$block.id")
		fi
	done
	[ -n "$cert" ] || die "the keychain did not export the identity $sha1"
	for block in "$dir"/*.key; do
		[ -f "$block" ] || continue
		if [ -n "$id" ] && [ "$(cat "$block.id")" = "$id" ]; then key=$block; fi
	done
	if [ -z "$key" ] && [ "$(find "$dir" -name '*.key' | wc -l | tr -d ' ')" = 1 ]; then
		key=$(find "$dir" -name '*.key')
	fi
	[ -n "$key" ] || die "the keychain did not export the private key of $sha1"
	[ "$(/usr/bin/openssl x509 -in "$cert" -noout -pubkey)" = "$(/usr/bin/openssl pkey -in "$key" -pubout)" ] ||
		die "the exported private key doesn't belong to the certificate $sha1"
	pw=$(/usr/bin/openssl rand -hex 24)
	KIWI_PW=$pw /usr/bin/openssl pkcs12 -export -in "$cert" -inkey "$key" -name "Kiwi" \
		-certpbe PBE-SHA1-3DES -keypbe PBE-SHA1-3DES -macalg sha1 -passout env:KIWI_PW -out "$p12" ||
		die "could not pack the identity as a .p12"
	printf '%s\n' "$pw" >"$password_file"
	rm -rf "$dir"
	good "Signing identity: $(/usr/bin/openssl x509 -in "$cert" -noout -subject | sed -n 's/.*CN *= *\([^/,]*\).*/\1/p')"
}

# Copies the provisioning profiles Xcode keeps (Kiwi on Linux takes the valid iOS ones).
copy_profiles() {
	local to=$1 dir profile count=0
	mkdir -p "$to"
	for dir in "$HOME/Library/Developer/Xcode/UserData/Provisioning Profiles" \
		"$HOME/Library/MobileDevice/Provisioning Profiles"; do
		for profile in "$dir"/*.mobileprovision; do
			[ -f "$profile" ] || continue
			cp "$profile" "$to/"
			count=$((count + 1))
		done
	done
	good "$count provisioning profile(s)"
}

export_main() {
	local ts status suffix login owner server host db kiwi_version team tailnet_name size free answer entries back
	while [ $# -gt 0 ]; do
		case "$1" in
		--name)
			[ $# -ge 2 ] || die "--name needs a name"
			NAME=$2
			shift
			;;
		--out)
			[ $# -ge 2 ] || die "--out needs a file"
			OUT=$2
			shift
			;;
		--builder-name)
			[ $# -ge 2 ] || die "--builder-name needs a name"
			BUILDER_NAME=$2
			shift
			;;
		--no-builder) BUILDER=0 ;;
		--trial) TRIAL=1 BUILDER=0 ;;
		--p12)
			[ $# -ge 2 ] || die "--p12 needs a file"
			P12=$2
			shift
			;;
		--p12-password-file)
			[ $# -ge 2 ] || die "--p12-password-file needs a file"
			P12_PASSWORD_FILE=$2
			shift
			;;
		--keychain)
			[ $# -ge 2 ] || die "--keychain needs a file"
			KEYCHAIN=$2
			shift
			;;
		--identity)
			[ $# -ge 2 ] || die "--identity needs a SHA-1"
			IDENTITY=$2
			shift
			;;
		--replace-builder) REPLACE_BUILDER=1 ;;
		--no-build) BUILD=0 ;;
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
	case "$NAME" in
	"" | -* | *[!a-z0-9-]*) die "--name must be a Tailscale machine name: lower-case letters, digits and -" ;;
	esac

	umask 077 # the bundle and everything made for it are private
	say "Checking this Mac"
	[ "$(uname -s)" = Darwin ] || die "export runs on the Mac where Kiwi runs now"
	[ "$(id -u)" != 0 ] || die "run this as yourself, not as root"
	[ -f "$DATA/kiwi.db" ] || die "no Kiwi on this Mac ($DATA/kiwi.db is missing)"
	for tool in jq sqlite3 shasum; do
		command -v "$tool" >/dev/null || die "$tool is missing"
	done
	good "Kiwi's data: $DATA"
	if [ "$TRIAL" = 0 ] && [ -z "$P12" ] && ! launchctl print "gui/$(id -u)" >/dev/null 2>&1; then
		die "no desktop session: run this in Terminal on the Mac's desktop (or Screen Sharing), where macOS can ask to allow exporting the signing identity"
	fi
	if [ -n "$P12" ]; then
		[ -f "$P12" ] || die "$P12 is missing"
	fi
	ts=$(find_tailscale) || die "Tailscale is missing"
	status=$("$ts" status --json) || die "Tailscale does not answer"
	suffix=$(jq -r '.MagicDNSSuffix // empty' <<<"$status")
	[ -n "$suffix" ] || die "MagicDNS is off in your tailnet (https://login.tailscale.com/admin/dns)"
	login=$(jq -r '.Self.UserID as $u | .User[($u | tostring)].LoginName // empty' <<<"$status")
	tailnet_name=$(jq -r '.Self.DNSName // empty' <<<"$status")
	tailnet_name=${tailnet_name%.}
	host="$NAME.$suffix"
	server="https://$host"
	if jq -e --arg h "$host." '[.Self, (.Peer // {})[]] | any(.DNSName == $h)' <<<"$status" >/dev/null; then
		die "$host is taken in your tailnet: remove that device at https://login.tailscale.com/admin/machines, or pick another --name"
	fi
	good "Kiwi's new address: $server"
	owner=$(sqlite3 "$DATA/kiwi.db" "SELECT value FROM meta WHERE key = 'owner_login'" 2>/dev/null || true)
	owner=${owner:-$login}
	[ -n "$owner" ] || die "could not tell Kiwi's owner (its Tailscale login)"
	good "Owner: $owner"
	if [ "$BUILDER" = 1 ]; then
		if [ -f "$B_DATA/builder.json" ] && [ "$REPLACE_BUILDER" = 0 ]; then
			die "this Mac's builder is already paired ($B_DATA/builder.json): pass --replace-builder to pair it with the new Kiwi, or --no-builder"
		fi
		[ -f "$REPO/crates/kiwi-server/Cargo.toml" ] || die "$REPO is not the Kiwi repository"
		if [ "$BUILD" = 1 ]; then
			command -v cargo >/dev/null || die "Rust is missing: install it from https://rustup.rs"
		else
			[ -x "$REPO/target/release/kiwi" ] || die "$REPO/target/release/kiwi is missing; run without --no-build"
		fi
	fi

	OUT=${OUT:-$HOME/kiwi-migration-$(date +%Y%m%d-%H%M).tar}
	[ ! -e "$OUT" ] || die "$OUT exists already"
	entries=$(data_entries)
	# shellcheck disable=SC2086 # one entry per line, no glob characters in Kiwi's names
	size=$(cd "$DATA" && IFS=$'\n' && du -skc $entries kiwi.db | tail -n 1 | cut -f 1)
	free=$(df -k "$(dirname "$OUT")" | awk 'NR == 2 { print $4 }')
	[ "$free" -gt $((size + 1048576)) ] || die "$(dirname "$OUT") has $((free / 1048576)) GB free, the bundle needs about $((size / 1048576 + 1)) GB: pass --out on another disk"
	good "Bundle: $OUT (about $((size / 1048576 + 1)) GB)"

	if [ "$TRIAL" = 0 ] && [ -t 0 ]; then
		printf '\nThis stops Kiwi on this Mac for good: the new Kiwi at %s takes over\n' "$server"
		printf 'after the import (docs/migrate.md says how to go back). Continue? [y/N] '
		read -r answer
		case "$answer" in y | Y | yes) ;; *) die "stopped; nothing changed" ;; esac
	fi

	WORK=$(mktemp -d "$(dirname "$OUT")/.kiwi-migration.XXXXXX")
	chmod 700 "$WORK"
	trap export_cleanup EXIT
	mkdir -p "$WORK/bundle/data" "$WORK/bundle/signing"

	if [ "$BUILDER" = 1 ] && [ "$BUILD" = 1 ]; then
		say "Building Kiwi for the Mac builder (cargo build --release)"
		(cd "$REPO" && cargo build --release --locked -p kiwi-server)
	fi

	say "Signing"
	if [ -n "$P12" ]; then
		cp "$P12" "$WORK/bundle/signing/identity.p12"
		if [ -n "$P12_PASSWORD_FILE" ]; then
			cp "$P12_PASSWORD_FILE" "$WORK/bundle/signing/identity.password"
		else
			printf 'Password of %s: ' "$P12"
			stty -echo 2>/dev/null || true
			IFS= read -r answer || true
			stty echo 2>/dev/null || true
			printf '\n'
			printf '%s\n' "$answer" >"$WORK/bundle/signing/identity.password"
		fi
		good "Signing identity: $P12"
	else
		export_identity "$(signing_sha1 "$DATA/kiwi.db")" "$WORK/bundle/signing/identity.p12" \
			"$WORK/bundle/signing/identity.password"
	fi
	copy_profiles "$WORK/bundle/signing/profiles"

	if [ "$TRIAL" = 0 ]; then
		say "Stopping Kiwi on this Mac"
		STOPPED=1
		launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
		launchctl disable "gui/$(id -u)/$LABEL"
		for _ in $(seq 1 30); do
			[ -n "$(mac_kiwi_pid)" ] || break
			sleep 1
		done
		[ -z "$(mac_kiwi_pid)" ] || die "Kiwi on this Mac did not stop"
		good "Stopped (and kept from starting at login)"
	fi

	say "Copying the database"
	db="$WORK/bundle/data/kiwi.db"
	(umask 077 && cd "$WORK/bundle/data" && sqlite3 "$DATA/kiwi.db" ".backup kiwi.db")
	[ "$(sqlite3 "$db" 'PRAGMA integrity_check')" = ok ] || die "the copy of kiwi.db is damaged"
	team=$(sqlite3 "$db" "SELECT value FROM meta WHERE key = 'team_id'" || true)
	good "kiwi.db (schema $(sqlite3 "$db" 'PRAGMA user_version'), $(sqlite3 "$db" 'SELECT count(*) FROM devices') devices, $(sqlite3 "$db" 'SELECT count(*) FROM sources') sources)"
	kiwi_version=$(curl -fsS --max-time 2 http://127.0.0.1:8742/api/health 2>/dev/null | jq -r '.version // empty' || true)
	if [ -z "$kiwi_version" ] && [ -x "$DATA/app/kiwi" ]; then
		kiwi_version=$("$DATA/app/kiwi" --version 2>/dev/null || true)
		kiwi_version=${kiwi_version##* }
	fi

	if [ "$BUILDER" = 1 ]; then
		say "Pairing this Mac's builder with $server ahead of time"
		set -- builder provision --data-dir "$B_DATA" --server "$server"
		if [ -n "$BUILDER_NAME" ]; then set -- "$@" --name "$BUILDER_NAME"; fi
		if [ "$REPLACE_BUILDER" = 1 ]; then set -- "$@" --replace; fi
		"$REPO/target/release/kiwi" "$@" >"$WORK/bundle/mac-builder.json"
		good "Mac builder: $(jq -r .name "$WORK/bundle/mac-builder.json")"
	fi

	jq -n --argjson format "$FORMAT" --arg created "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
		--arg data "$DATA" --arg version "$kiwi_version" --arg old "$tailnet_name" \
		--arg name "$NAME" --arg url "$server" --arg owner "$owner" --arg team "$team" \
		--argjson trial "$([ "$TRIAL" = 1 ] && echo true || echo false)" \
		'{format: $format, created_at: $created, trial: $trial,
		  source: {data_dir: $data, kiwi_version: $version, tailscale_name: $old},
		  hostname: $name, public_url: $url, owner: $owner, team_id: $team}' \
		>"$WORK/bundle/manifest.json"

	say "Packing $OUT"
	# No extended attributes or AppleDouble (._*) files: Linux has no use for them.
	export COPYFILE_DISABLE=1
	set -- manifest.json signing data
	if [ "$BUILDER" = 1 ]; then set -- "$@" mac-builder.json; fi
	(umask 077 && tar --no-xattrs --no-mac-metadata --no-acls -cf "$OUT.partial" -C "$WORK/bundle" "$@")
	# The data directory itself, under data/ (symbolic links keep their targets).
	# shellcheck disable=SC2086 # one entry per line, no glob characters in Kiwi's names
	(cd "$DATA" && IFS=$'\n' && tar --no-xattrs --no-mac-metadata --no-acls -rf "$OUT.partial" -s ',^,data/,S' $entries)
	mv "$OUT.partial" "$OUT"
	chmod 600 "$OUT"
	(cd "$(dirname "$OUT")" && shasum -a 256 "$(basename "$OUT")" >"$OUT.sha256")
	DONE=1
	good "$OUT ($(du -h "$OUT" | cut -f 1 | tr -d ' '))"

	if [ "$BUILDER" = 1 ]; then
		say "Installing the Mac builder"
		if ! "$REPO/install.sh" --builder --no-build --import-enrollment "$DATA"; then
			printf '\nmigrate: the Mac builder did not install (above); the bundle is complete. Fix that and run:\n  %s/install.sh --builder --no-build --import-enrollment\n' "$REPO" >&2
		fi
	fi

	cat <<EOF

Kiwi is packed in $OUT
$(if [ "$TRIAL" = 1 ]; then echo "(a trial copy: Kiwi keeps running on this Mac)"; else echo "Kiwi on this Mac is stopped; its data stays in $DATA."; fi)

Next:
  1. Copy the bundle to the Proxmox host (it holds your signing keys; delete it afterwards):
       scp "$OUT" "$OUT.sha256" root@<proxmox>:/root/
  2. There, create Kiwi's container (docs/migrate.md):
       ./deploy/proxmox/kiwi-lxc.sh --bundle /root/$(basename "$OUT")
     or on a Linux computer that has Kiwi: sudo ./migrate.sh import $(basename "$OUT")
  3. Open $server/ on your phones: add it to the Home Screen again and turn on notifications.
EOF
	if [ "$TRIAL" = 0 ]; then
		back="launchctl enable gui/$(id -u)/$LABEL && launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/$LABEL.plist"
		printf '\nGo back to Kiwi on this Mac (before the new Kiwi changes anything):\n  %s\n' "$back"
	fi
}

# ---------------------------------------------------------------------------------------
# Import (Linux)

L_DATA=/var/lib/kiwi
UNIT=/etc/systemd/system/kiwi.service
APP=/opt/kiwi/app
SERVICE_USER=kiwi
REPLACE=0
STAGING= # the unpacked bundle, removed on exit

unit_arg() {
	awk -v key="--$1" '/^ExecStart=/ { for (i = 1; i < NF; i++) if ($i == key) { v = $(i + 1); gsub(/"/, "", v); print v; exit } }' "$UNIT"
}

# Whether the data directory holds a Kiwi someone used: one with devices or signed apps
# (not just the one ./install.sh started).
used_data() {
	[ -f "$L_DATA/kiwi.db" ] || return 1
	[ "$(sqlite3 "$L_DATA/kiwi.db" 'SELECT (SELECT count(*) FROM devices) + (SELECT count(*) FROM builds)' 2>/dev/null || echo 1)" != 0 ]
}

wait_healthy() {
	local port=$1 health
	for _ in $(seq 1 90); do
		if health=$(curl -fsS --max-time 2 "http://127.0.0.1:$port/api/health" 2>/dev/null); then
			sleep 5
			if [ "$(systemctl is-active kiwi.service)" = active ] &&
				health=$(curl -fsS --max-time 2 "http://127.0.0.1:$port/api/health" 2>/dev/null); then
				printf '%s\n' "$health"
				return 0
			fi
		fi
		sleep 1
	done
	systemctl status --no-pager --lines=0 kiwi.service >&2 || true
	journalctl -u kiwi.service -n 40 --no-pager >&2 || true
	return 1
}

import_main() {
	local bundle='' staging='' aside='' manifest url port expected owner stamp line ts status self_owner probe entry
	while [ $# -gt 0 ]; do
		case "$1" in
		--replace) REPLACE=1 ;;
		-h | --help)
			usage
			return 0
			;;
		-*)
			usage >&2
			return 2
			;;
		*)
			[ -z "$bundle" ] || die "one bundle at a time"
			bundle=$1
			;;
		esac
		shift
	done
	[ -n "$bundle" ] || die "name the bundle: ./migrate.sh import kiwi-migration-<date>.tar"
	[ "$(uname -s)" = Linux ] || die "import runs on the Linux computer Kiwi moves to"
	if [ "$(id -u)" != 0 ]; then
		say "Importing as root (sudo)"
		set -- "$(cd "$(dirname "$bundle")" && pwd)/$(basename "$bundle")"
		if [ "$REPLACE" = 1 ]; then set -- --replace "$@"; fi
		exec sudo "$BASH" "$0" import "$@"
	fi
	for tool in jq sqlite3 curl tar runuser systemctl; do
		command -v "$tool" >/dev/null || die "$tool is missing"
	done
	{ [ -f "$UNIT" ] && [ -x "$APP/kiwi" ]; } || die "install Kiwi first: ./install.sh (then run this again)"
	[ -e "$bundle" ] || die "$bundle is missing"

	# A bundle file, or one unpacked into a directory (kiwi-lxc.sh streams it into one).
	if [ -d "$bundle" ]; then
		{ manifest=$(cat "$bundle/manifest.json" 2>/dev/null) && [ -f "$bundle/data/kiwi.db" ]; } ||
			die "$bundle is not a Kiwi bundle (migrate.sh export makes one)"
	else
		if [ -f "$bundle.sha256" ]; then
			(cd "$(dirname "$bundle")" && sha256sum -c --quiet "$(basename "$bundle").sha256") ||
				die "$bundle is damaged (its SHA-256 doesn't match $bundle.sha256)"
			good "Checksum matches"
		fi
		manifest=$(tar -xOf "$bundle" --occurrence=1 manifest.json 2>/dev/null) ||
			die "$bundle is not a Kiwi bundle (migrate.sh export makes one)"
	fi
	[ "$(jq -r .format <<<"$manifest")" = "$FORMAT" ] || die "this bundle needs a newer Kiwi: git pull && ./install.sh"
	expected=$(jq -r '.public_url // empty' <<<"$manifest")
	owner=$(jq -r '.owner // empty' <<<"$manifest")
	good "From $(jq -r '.source.tailscale_name // "a Mac"' <<<"$manifest"), made $(jq -r .created_at <<<"$manifest")"

	url=$(unit_arg public-url)
	port=$(unit_arg listen)
	port=${port##*:}
	if [ -n "$expected" ] && [ "$url" != "$expected" ]; then
		note "Kiwi here is $url, the bundle was made for $expected: the Mac builder and your phones look for $expected"
		note "Rename this computer in Tailscale to $(jq -r .hostname <<<"$manifest") (https://login.tailscale.com/admin/machines) and run ./install.sh again"
	fi
	if ! grep -q -- '--owner' "$UNIT" && [ -n "$owner" ] && ts=$(find_tailscale) && status=$("$ts" status --json 2>/dev/null); then
		self_owner=$(jq -r '.Self.UserID as $u | .User[($u | tostring)].LoginName // empty' <<<"$status")
		if [ "$(jq -r '.Self.Tags // [] | length' <<<"$status")" != 0 ] || [ "$self_owner" != "$owner" ]; then
			note "This computer isn't $owner's in Tailscale: run ./install.sh --owner $owner so Kiwi knows its owner"
		fi
	fi

	if used_data && [ "$REPLACE" = 0 ]; then
		die "Kiwi here already has data ($L_DATA): pass --replace to put it aside and import anyway"
	fi

	# Unpacked next to the data directory (one file system, private), where Kiwi's user
	# can read it.
	say "Unpacking $bundle"
	stamp=$(date +%Y%m%d-%H%M%S)
	staging=$(mktemp -d "$(dirname "$L_DATA")/.kiwi-import.XXXXXX")
	STAGING=$staging
	trap 'rm -rf "$STAGING"' EXIT
	if [ -d "$bundle" ]; then
		for entry in manifest.json signing mac-builder.json; do
			if [ -e "$bundle/$entry" ]; then cp -R "$bundle/$entry" "$staging/"; fi
		done
		mv "$bundle/data" "$staging/data"
	else
		tar -xf "$bundle" -C "$staging" --no-same-owner
	fi

	say "Stopping Kiwi"
	systemctl stop kiwi.service
	aside="$L_DATA.before-import-$stamp"
	if [ -e "$L_DATA" ]; then mv "$L_DATA" "$aside"; fi
	mv "$staging/data" "$L_DATA"
	chown -R "$SERVICE_USER:$SERVICE_USER" "$L_DATA"
	chmod -R go-rwx "$L_DATA"
	# What the import reads besides the data directory, for Kiwi's user.
	chown -R "$SERVICE_USER:$SERVICE_USER" "$staging"
	chmod 700 "$staging"

	say "Adjusting Kiwi to this computer"
	if ! runuser -u "$SERVICE_USER" -- env NO_COLOR=1 "$APP/kiwi" migrate import --data-dir "$L_DATA" --bundle "$staging" >"$staging/report" 2>&1; then
		sed 's/^/  /' "$staging/report" >&2
		mv "$L_DATA" "$L_DATA.failed-import-$stamp"
		if [ -e "$aside" ]; then mv "$aside" "$L_DATA"; fi
		systemctl start kiwi.service || true
		die "the import failed (above); Kiwi is back as it was, the imported files are in $L_DATA.failed-import-$stamp"
	fi
	while IFS= read -r line; do good "$line"; done <"$staging/report"
	rm -rf "$staging"

	say "Starting Kiwi"
	systemctl start kiwi.service
	wait_healthy "$port" >/dev/null || die "Kiwi did not become healthy with the imported data (log above); the data that was here is in $aside"
	good "Kiwi runs at $url/"
	if [ -e "$aside" ]; then
		note "The data that was here before is in $aside: delete it once all is well"
	fi
	# What community builds' sandbox finds here, as Kiwi's service.
	if probe=$(systemd-run --quiet --wait --pipe --collect -p User="$SERVICE_USER" \
		-p NoNewPrivileges=yes -p RestrictNamespaces=yes \
		python3 -I "$L_DATA/automation/sandbox.py" --probe 2>/dev/null); then
		if [ "$(jq -r '.landlock // 0' <<<"$probe")" -ge 3 ] && [ "$(jq -r .seccomp <<<"$probe")" = true ]; then
			good "Community builds' sandbox: Landlock ABI $(jq -r .landlock <<<"$probe"), seccomp"
		else
			note "Community builds' sandbox is unavailable here: $(jq -r '.reasons | join("; ")' <<<"$probe")"
		fi
	fi
	cat <<EOF

Kiwi moved here. Delete the bundle: it holds your signing keys.
On your phones, open $url/ (Safari on an iPhone: Share › Add to Home Screen) and turn on
notifications again in Settings. The Mac builder connects by itself within a minute.
EOF
}

case "${1:-}" in
export)
	shift
	export_main "$@"
	;;
import)
	shift
	import_main "$@"
	;;
-h | --help)
	usage
	;;
*)
	usage >&2
	exit 2
	;;
esac
