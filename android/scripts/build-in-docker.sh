#!/usr/bin/env bash
# Builds and tests the Android app in a container, for hosts without an Android SDK.
#
#   android/scripts/build-in-docker.sh                      # test, debug and unsigned release APKs
#   android/scripts/build-in-docker.sh test                 # any Gradle tasks
#
# The image is pinned by digest (Android SDK 36, build-tools 36.0.0, OpenJDK 21). Gradle and the
# SDK packages the build adds (the compileSdk platform) are cached in android/.cache (ignored by
# git). Gradle runs as the calling user, so the outputs belong to you:
#
#   android/app/build/outputs/apk/debug/app-debug.apk
#   android/app/build/outputs/apk/release/app-release-unsigned.apk
#
# How the container is run:
#
# - KIWI_BUILD_HELPER=<command>: through an `agent-resource`-compatible helper that runs on
#   another machine (for example a wrapper that calls it over SSH), for a host whose containers
#   have no internet access. The sources go to the task on stdin and the build outputs
#   (app/build/outputs, reports, test-results) come back as a tar on stdout. Nothing is cached
#   between runs there.
# - Else, where `agent-resource` exists (a shared build host), through it: a lease with a deadline
#   that removes the container when the build ends or fails.
# - Else with `docker run --rm`.
set -euo pipefail

IMAGE="ghcr.io/cirruslabs/android-sdk:36@sha256:f9b3ea9ed2b5fc9522adae82c7b4622ab7aa54207ef532c8e615a347dca08f31"
CPUS="${KIWI_BUILD_CPUS:-8}"
MEMORY="${KIWI_BUILD_MEMORY:-12g}"
TASK="Kiwi Android app build"

android_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [ "$#" -eq 0 ]; then
  set -- test assembleDebug assembleRelease
fi

# Runs inside the container as root: prepares a writable copy of the image's SDK in the cache (the
# build adds the compileSdk platform to it), then runs Gradle as KIWI_UID:KIWI_GID.
read -r -d '' inner <<'INNER' || true
set -euo pipefail
cache=/work/.cache
mkdir -p "$cache/home" "$cache/gradle"
if [ ! -f "$cache/sdk/.kiwi-sdk" ]; then
  rm -rf "$cache/sdk"
  mkdir -p "$cache/sdk"
  for part in licenses build-tools platform-tools platforms cmdline-tools; do
    cp -a "$ANDROID_HOME/$part" "$cache/sdk/"
  done
  touch "$cache/sdk/.kiwi-sdk"
fi
chown -R "$KIWI_UID:$KIWI_GID" "$cache"
cd /work
exec setpriv --reuid="$KIWI_UID" --regid="$KIWI_GID" --clear-groups \
  env HOME="$cache/home" GRADLE_USER_HOME="$cache/gradle" ANDROID_HOME="$cache/sdk" \
  ANDROID_SDK_ROOT="$cache/sdk" ANDROID_USER_HOME="$cache/home/.android" \
  ./gradlew --no-daemon --console=plain "$@"
INNER

# The helper task: unpacks the sources from stdin into its workspace, runs the container, follows
# its log on stderr and writes the outputs to stdout as a tar.
# shellcheck disable=SC2016 # expanded by the task's shell
remote='set -euo pipefail
work="$AGENT_RESOURCE_WORKSPACE/android"
mkdir -p "$work"
tar -C "$work" -xf -
image=$1 inner=$2 cpus=$3 memory=$4
shift 4
cid=$(agent-resource docker-run --cpus "$cpus" --memory "$memory" --volume "$work:/work" \
  --env "KIWI_UID=$(id -u)" --env "KIWI_GID=$(id -g)" "$image" bash -c "$inner" kiwi-gradle "$@")
docker logs --follow "$cid" >&2 &
status=$(docker wait "$cid")
wait
cd "$work"
outputs=()
for path in app/build/outputs app/build/reports app/build/test-results; do
  if [ -e "$path" ]; then outputs+=("$path"); fi
done
if [ "${#outputs[@]}" -gt 0 ]; then tar -cf - "${outputs[@]}"; fi
exit "$status"'

if [ -n "${KIWI_BUILD_HELPER:-}" ]; then
  results="$(mktemp)"
  trap 'rm -f "$results"' EXIT
  status=0
  tar -C "$android_dir" --exclude=./.cache --exclude=./.gradle --exclude=./.kotlin --exclude=./build \
    --exclude=./app/build --exclude=./local.properties -cf - . |
    "$KIWI_BUILD_HELPER" run --task "$TASK" --ttl 2h -- \
      bash -c "$remote" kiwi-build "$IMAGE" "$inner" "$CPUS" "$MEMORY" "$@" >"$results" || status=$?
  if [ -s "$results" ]; then
    rm -rf "$android_dir/app/build/outputs" "$android_dir/app/build/reports" "$android_dir/app/build/test-results"
    mkdir -p "$android_dir/app/build"
    tar -C "$android_dir" -xf "$results"
  fi
  exit "$status"
fi

run_args=(--cpus "$CPUS" --memory "$MEMORY" --volume "$android_dir:/work"
  --env "KIWI_UID=$(id -u)" --env "KIWI_GID=$(id -g)")

if command -v agent-resource >/dev/null 2>&1; then
  # The supervised task owns the container: `docker-run` starts it detached; the task follows its
  # log, returns its exit status, and the helper removes the container afterwards.
  # shellcheck disable=SC2016 # expanded by the task's shell
  exec agent-resource run --task "$TASK" --ttl 2h -- \
    bash -c 'set -euo pipefail
      cid=$(agent-resource docker-run "$@")
      docker logs --follow "$cid" &
      status=$(docker wait "$cid")
      wait
      exit "$status"' kiwi-build "${run_args[@]}" "$IMAGE" bash -c "$inner" kiwi-gradle "$@"
fi

exec docker run --rm "${run_args[@]}" "$IMAGE" bash -c "$inner" kiwi-gradle "$@"
