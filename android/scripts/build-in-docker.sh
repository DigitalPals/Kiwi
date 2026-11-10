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
# - KIWI_BUILD_HELPER=<command>: through your own helper, for a host whose containers have no
#   internet access or that has no Docker. It is called as
#     <command> IMAGE SCRIPT CPUS MEMORY [GRADLE ARGS...]
#   with the Android sources as a tar on stdin. It runs `bash -c SCRIPT kiwi-gradle [GRADLE ARGS...]`
#   in IMAGE (as root, with the sources at /work and KIWI_UID/KIWI_GID set to the user that should
#   own the outputs, at most CPUS and MEMORY), for example on another machine over SSH, and writes
#   app/build/outputs, app/build/reports and app/build/test-results as a tar on stdout. Its exit
#   status is the build's.
# - Else with `docker run --rm`.
set -euo pipefail

IMAGE="ghcr.io/cirruslabs/android-sdk:36@sha256:f9b3ea9ed2b5fc9522adae82c7b4622ab7aa54207ef532c8e615a347dca08f31"
CPUS="${KIWI_BUILD_CPUS:-8}"
MEMORY="${KIWI_BUILD_MEMORY:-12g}"

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

if [ -n "${KIWI_BUILD_HELPER:-}" ]; then
  results="$(mktemp)"
  trap 'rm -f "$results"' EXIT
  status=0
  tar -C "$android_dir" --exclude=./.cache --exclude=./.gradle --exclude=./.kotlin --exclude=./build \
    --exclude=./app/build --exclude=./local.properties -cf - . |
    "$KIWI_BUILD_HELPER" "$IMAGE" "$inner" "$CPUS" "$MEMORY" "$@" >"$results" || status=$?
  if [ -s "$results" ]; then
    rm -rf "$android_dir/app/build/outputs" "$android_dir/app/build/reports" "$android_dir/app/build/test-results"
    mkdir -p "$android_dir/app/build"
    tar -C "$android_dir" -xf "$results"
  fi
  exit "$status"
fi

run_args=(--cpus "$CPUS" --memory "$MEMORY" --volume "$android_dir:/work"
  --env "KIWI_UID=$(id -u)" --env "KIWI_GID=$(id -g)")

exec docker run --rm "${run_args[@]}" "$IMAGE" bash -c "$inner" kiwi-gradle "$@"
