#!/bin/bash
set -euo pipefail
root=$(cd -- "$(dirname -- "$0")" && pwd)
output=${1:?Usage: build-gboard.sh OUTPUT [iphoneos|iphonesimulator]}
platform=${2:-iphoneos}
export DEVELOPER_DIR=${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}
case "$platform" in
    iphoneos) target=arm64-apple-ios16.0 ;;
    iphonesimulator) target=arm64-apple-ios16.0-simulator ;;
    *) echo 'Choose iphoneos or iphonesimulator.' >&2; exit 2 ;;
esac
mkdir -p "$output"
output=$(cd -- "$output" && pwd)
sdk=$(xcrun --sdk "$platform" --show-sdk-path)
sources=("$root/Sources/RamblerCore.swift" "$root/Sources/OggOpus.swift" "$root/Gboard/SpeechActivity.swift" "$root/Gboard/Session.swift")
common=(-sdk "$sdk" -target "$target" -parse-as-library -module-name KiwiGboardRambler -import-objc-header "$root/Gboard/Bridge.h")
xcrun --sdk "$platform" swiftc "${common[@]}" "${sources[@]}" -emit-module \
    -emit-module-path "$output/KiwiGboardRambler.swiftmodule" -emit-objc-header -emit-objc-header-path "$output/KiwiGboardRambler-Swift.h"
xcrun --sdk "$platform" clang -target "$target" -isysroot "$sdk" -fobjc-arc -fmodules \
    -Wall -Werror=return-type -Wno-nullability-completeness -Wno-incompatible-pointer-types \
    -I "$output" -I "$root/Gboard" -c "$root/Gboard/Hooks.m" -o "$output/hooks.o"
xcrun --sdk "$platform" swiftc "${common[@]}" -O "${sources[@]}" "$output/hooks.o" \
    -emit-library -Xlinker -install_name -Xlinker @rpath/KiwiGboardRambler.dylib -o "$output/KiwiGboardRambler.dylib"
codesign --force --sign - "$output/KiwiGboardRambler.dylib" >/dev/null
