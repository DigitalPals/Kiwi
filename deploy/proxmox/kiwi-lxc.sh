#!/bin/bash
# Create Kiwi's container on a Proxmox VE host and move Kiwi into it from a Mac
# (docs/migrate.md). Run as root on the Proxmox host:
#
#   ./kiwi-lxc.sh --bundle /root/kiwi-migration-<date>.tar [options]
#
# It creates an unprivileged Debian 13 container (nesting on, for the service's sandbox and
# community builds; /dev/net/tun for Tailscale) that starts with the host, installs
# Tailscale and signs it in as the bundle's name (an auth key, or a login link it prints),
# builds and installs Kiwi from GitHub with ./install.sh, imports the bundle with
# ./migrate.sh import and checks that Kiwi answers at its new address. Without --bundle it
# sets up a new, empty Kiwi (pass --name).
#
# Options:
#   --bundle FILE        the bundle from ./migrate.sh export on the Mac
#   --authkey-file FILE  a Tailscale auth key (https://login.tailscale.com/admin/settings/keys,
#                        not ephemeral, not tagged): no login link to open
#   --name NAME          the Tailscale and host name [default: the bundle's, else kiwi]
#   --ctid ID            the container's id [default: the next free one]
#   --storage STORAGE    where its disk goes [default: local-lvm, local-zfs or the first
#                        storage for containers]
#   --disk GB            its disk [default: 64]
#   --cores N            [default: 4]
#   --memory MB          [default: 8192]
#   --bridge BRIDGE      [default: vmbr0]
#   --ip CIDR --gw IP    a fixed address [default: DHCP]
#   --vlan TAG           a VLAN tag on the bridge
#   --ref REF            the Kiwi branch or tag to install [default: main]
#   --repo URL           [default: https://github.com/DigitalPals/Kiwi.git]
set -euo pipefail

BUNDLE=
AUTHKEY_FILE=
NAME=
CTID=
STORAGE=
DISK=64
CORES=4
MEMORY=8192
BRIDGE=vmbr0
IP=dhcp
GW=
VLAN=
REF=main
REPO_URL=https://github.com/DigitalPals/Kiwi.git
SRC=/root/kiwi       # the clone in the container (updates: cd /root/kiwi && git pull && ./install.sh)
IMPORT=/var/lib/.kiwi-import

say() { printf '==> %s\n' "$*"; }
good() { printf '  \342\234\223 %s\n' "$*"; }
note() { printf '  - %s\n' "$*"; }
die() {
	printf 'kiwi-lxc: %s\n' "$*" >&2
	exit 1
}
usage() { sed -n '2,33p' "$0" | sed 's/^# \{0,1\}//'; }

# Runs a command in the container.
in_ct() { pct exec "$CTID" -- "$@"; }

# Runs a bash script (stdin) in the container, with Kiwi's environment.
in_ct_script() {
	pct exec "$CTID" -- env DEBIAN_FRONTEND=noninteractive LC_ALL=C.UTF-8 HOME=/root \
		PATH=/root/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
		bash -euo pipefail -s -- "$@"
}

manifest() { jq -r "$1 // empty" <<<"$MANIFEST"; }

CREATED=0 # the container exists
DONE=0
on_exit() {
	if [ "$CREATED" = 1 ] && [ "$DONE" = 0 ]; then
		printf '\nkiwi-lxc: container %s stays as it is, to look at (pct enter %s). Remove it before trying again: pct stop %s; pct destroy %s\n' \
			"$CTID" "$CTID" "$CTID" "$CTID" >&2
	fi
}
trap on_exit EXIT

main() {
	while [ $# -gt 0 ]; do
		case "$1" in
		--bundle | --authkey-file | --name | --ctid | --storage | --disk | --cores | --memory | --bridge | --ip | --gw | --vlan | --ref | --repo)
			[ $# -ge 2 ] || die "$1 needs a value"
			case "$1" in
			--bundle) BUNDLE=$2 ;;
			--authkey-file) AUTHKEY_FILE=$2 ;;
			--name) NAME=$2 ;;
			--ctid) CTID=$2 ;;
			--storage) STORAGE=$2 ;;
			--disk) DISK=$2 ;;
			--cores) CORES=$2 ;;
			--memory) MEMORY=$2 ;;
			--bridge) BRIDGE=$2 ;;
			--ip) IP=$2 ;;
			--gw) GW=$2 ;;
			--vlan) VLAN=$2 ;;
			--ref) REF=$2 ;;
			--repo) REPO_URL=$2 ;;
			esac
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

	say "Checking this Proxmox host"
	[ "$(id -u)" = 0 ] || die "run this as root on the Proxmox host"
	for tool in pct pveam pvesm pvesh; do
		command -v "$tool" >/dev/null || die "$tool is missing: run this on a Proxmox VE host"
	done
	command -v jq >/dev/null || apt-get install -y -qq jq >/dev/null || die "jq is missing (apt install jq)"
	good "$(pveversion 2>/dev/null || echo "Proxmox VE"), kernel $(uname -r)"
	local major minor
	IFS=. read -r major minor _ <<<"$(uname -r)"
	if [ "$major" -lt 6 ] || { [ "$major" = 6 ] && [ "$minor" -lt 2 ]; }; then
		note "Kernel $(uname -r) has no Landlock (6.2 or later): community builds can't run sandboxed in the container"
	fi

	MANIFEST='{}'
	if [ -n "$BUNDLE" ]; then
		[ -f "$BUNDLE" ] || die "$BUNDLE is missing"
		if [ -f "$BUNDLE.sha256" ]; then
			say "Checking the bundle"
			(cd "$(dirname "$BUNDLE")" && sha256sum -c --quiet "$(basename "$BUNDLE").sha256") ||
				die "$BUNDLE is damaged (its SHA-256 doesn't match $BUNDLE.sha256): copy it again"
			good "Checksum matches"
		fi
		MANIFEST=$(tar -xOf "$BUNDLE" --occurrence=1 manifest.json 2>/dev/null) ||
			die "$BUNDLE is not a Kiwi bundle (./migrate.sh export makes one)"
		[ "$(manifest .format)" = 1 ] || die "$BUNDLE needs a newer version of this script"
		good "Bundle from $(manifest .source.tailscale_name), made $(manifest .created_at), for $(manifest .public_url)"
		if [ "$(manifest .trial)" = true ]; then
			note "A trial copy: Kiwi still runs on the Mac. Both send notifications until you stop one"
		fi
		NAME=${NAME:-$(manifest .hostname)}
		if [ -n "$(manifest .hostname)" ] && [ "$NAME" != "$(manifest .hostname)" ]; then
			note "The bundle was made for $(manifest .hostname): the Mac builder and your phones will look for that name"
		fi
	fi
	NAME=${NAME:-kiwi}
	case "$NAME" in
	"" | -* | *[!a-z0-9-]*) die "--name must be lower-case letters, digits and -" ;;
	esac
	if [ -n "$AUTHKEY_FILE" ]; then
		[ -s "$AUTHKEY_FILE" ] || die "$AUTHKEY_FILE is missing or empty"
	fi

	CTID=${CTID:-$(pvesh get /cluster/nextid)}
	! pct status "$CTID" >/dev/null 2>&1 || die "container $CTID exists already"
	if [ -z "$STORAGE" ]; then
		local available
		available=$(pvesm status --content rootdir --enabled 1 2>/dev/null | awk 'NR > 1 { print $1 }')
		for STORAGE in local-lvm local-zfs $available; do
			grep -qx "$STORAGE" <<<"$available" && break
		done
		grep -qx "$STORAGE" <<<"$available" || die "no storage for containers; name one with --storage"
	fi
	local free
	free=$(pvesm status --storage "$STORAGE" 2>/dev/null | awk 'NR == 2 { print int($6 / 1048576) }')
	if [ -n "$free" ] && [ "$free" -lt "$DISK" ]; then
		die "$STORAGE has $free GB free, the container's disk is $DISK GB: pick --storage or a smaller --disk"
	fi
	good "Container $CTID \"$NAME\": $CORES cores, $MEMORY MB, $DISK GB on $STORAGE, $BRIDGE"

	say "Debian 13 template"
	pveam update >/dev/null || note "pveam update failed; using the templates known so far"
	local template
	template=$(pveam available --section system | awk '$2 ~ /^debian-13-standard_.*_amd64\.tar\.(zst|gz|xz)$/ { print $2 }' | sort -V | tail -n 1)
	[ -n "$template" ] || die "Proxmox offers no Debian 13 template: update Proxmox VE (8.4 or later)"
	if ! pveam list local | grep -q "$template"; then
		pveam download local "$template" >/dev/null
	fi
	good "$template"

	say "Creating container $CTID"
	local net="name=eth0,bridge=$BRIDGE,ip=$IP"
	if [ -n "$GW" ]; then net="$net,gw=$GW"; fi
	if [ -n "$VLAN" ]; then net="$net,tag=$VLAN"; fi
	pct create "$CTID" "local:vztmpl/$template" \
		--hostname "$NAME" --description "Kiwi, a private app store (https://github.com/DigitalPals/Kiwi)" \
		--tags kiwi --unprivileged 1 --features nesting=1 --ostype debian \
		--cores "$CORES" --memory "$MEMORY" --swap 2048 \
		--rootfs "$STORAGE:$DISK" --net0 "$net" --onboot 1 --timezone host >/dev/null
	# Tailscale needs /dev/net/tun.
	cat >>"/etc/pve/lxc/$CTID.conf" <<EOF
lxc.cgroup2.devices.allow: c 10:200 rwm
lxc.mount.entry: /dev/net/tun dev/net/tun none bind,create=file
EOF
	CREATED=1
	pct start "$CTID"
	for _ in $(seq 1 60); do
		if in_ct getent hosts deb.debian.org >/dev/null 2>&1; then break; fi
		sleep 2
	done
	in_ct getent hosts deb.debian.org >/dev/null || die "container $CTID has no network (check --bridge, --ip)"
	good "Started; it starts with the host"

	say "Installing packages"
	in_ct_script <<'EOF'
apt-get update -qq
apt-get install -y -qq ca-certificates curl git jq sqlite3 build-essential pkg-config \
	python3 nodejs npm >/dev/null
curl -fsSL https://tailscale.com/install.sh | sh >/dev/null
systemctl enable --now tailscaled >/dev/null 2>&1
# Rust as pinned in rust-toolchain.toml, installed on first use.
curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none >/dev/null
EOF
	good "Tailscale, Rust, Node.js $(in_ct node --version), Python $(in_ct python3 -c 'import platform; print(platform.python_version())')"

	say "Signing in to Tailscale as $NAME"
	if [ -n "$AUTHKEY_FILE" ]; then
		pct push "$CTID" "$AUTHKEY_FILE" /root/.tailscale-authkey --perms 600
		in_ct_script "$NAME" <<'EOF'
trap 'rm -f /root/.tailscale-authkey' EXIT
tailscale up --hostname="$1" --auth-key=file:/root/.tailscale-authkey
EOF
	else
		note "Open the link Tailscale prints to add the container to your tailnet"
		in_ct tailscale up --hostname="$NAME"
	fi
	local status dns
	status=$(in_ct tailscale status --json)
	dns=$(jq -r '.Self.DNSName // empty' <<<"$status")
	dns=${dns%.}
	good "Connected as $dns"
	if [ -n "$(manifest .public_url)" ] && [ "https://$dns" != "$(manifest .public_url)" ]; then
		die "the container became $dns, but the bundle is for $(manifest .public_url): remove the device that has that name (https://login.tailscale.com/admin/machines), rename this one to $(manifest .hostname) there, then run in the container: cd $SRC && ./install.sh --owner $(manifest .owner) && ./migrate.sh import $IMPORT"
	fi

	if [ -n "$BUNDLE" ]; then
		say "Copying the bundle into the container"
		in_ct install -d -m 700 "$IMPORT"
		pct exec "$CTID" -- tar -xf - -C "$IMPORT" --no-same-owner <"$BUNDLE"
		good "Unpacked in $IMPORT"
	fi

	say "Building and installing Kiwi ($REF; this takes a while)"
	local owner
	owner=$(manifest .owner)
	in_ct_script "$REPO_URL" "$REF" "$SRC" "$owner" <<'EOF'
git clone --quiet --depth 1 --branch "$2" "$1" "$3"
cd "$3"
# The toolchain rust-toolchain.toml pins (older rustup: rustup show installs it).
rustup toolchain install >/dev/null 2>&1 || rustup show >/dev/null
if [ -n "$4" ]; then ./install.sh --owner "$4"; else ./install.sh; fi
EOF

	if [ -n "$BUNDLE" ]; then
		say "Moving Kiwi in"
		in_ct_script "$SRC" "$IMPORT" <<'EOF'
cd "$1"
./migrate.sh import "$2"
rm -rf "$2"
EOF
	fi

	local url="https://$dns"
	say "Checking $url from the container"
	local ip
	ip=$(in_ct tailscale ip -4 | head -n 1)
	if in_ct curl -fsS --max-time 30 --resolve "$dns:443:$ip" "$url/api/health" >/dev/null; then
		good "Kiwi answers at $url/"
	else
		note "Kiwi doesn't answer at $url/ yet (Tailscale may still be getting its certificate); try again in a minute"
	fi

	DONE=1
	cat <<EOF

Kiwi runs in container $CTID at $url/ (tailnet only).

$(if [ -n "$BUNDLE" ]; then
		cat <<EOT
Next:
  - Delete the bundle here and on the Mac: it holds your signing keys.
  - On your phones, open $url/ (Safari on an iPhone: Share › Add to Home Screen) and turn on
    notifications again in Settings.
  - The Mac builder connects by itself; Settings › Mac builder shows it.
EOT
	else
		echo "Next: open $url/ on your phone (docs/install.md)."
	fi)

Update Kiwi: pct exec $CTID -- bash -c 'cd $SRC && git pull && ./install.sh'
Log:         pct exec $CTID -- journalctl -u kiwi -f
EOF
}

main "$@"
