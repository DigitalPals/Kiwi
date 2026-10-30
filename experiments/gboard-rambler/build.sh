#!/bin/bash
set -euo pipefail

root=$(cd -- "$(dirname -- "$0")" && pwd)
mode=${1:?Usage: build.sh cli|test|simulator|device OUTPUT [CALLER.json SAMPLE.wav]}
output=${2:?Supply a private output directory}
export DEVELOPER_DIR=${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}
mkdir -p "$output"
output=$(cd -- "$output" && pwd)

if [[ "$mode" == cli || "$mode" == test ]]; then
    sdk=$(xcrun --sdk macosx --show-sdk-path)
    sources=("$root/Sources/RamblerCore.swift")
    if [[ "$mode" == cli ]]; then
        sources+=("$root/Sources/OggOpus.swift" "$root/Sources/CLI.swift")
    else
        sources+=("$root/Tests/ProtocolTests.swift")
    fi
    xcrun --sdk macosx swiftc -sdk "$sdk" -target arm64-apple-macosx14.0 -parse-as-library "${sources[@]}" -o "$output/rambler-$mode"
    if [[ "$mode" == test ]]; then "$output/rambler-test"; fi
    exit 0
fi

case "$mode" in
    simulator) platform=iphonesimulator; target=arm64-apple-ios16.0-simulator ;;
    device) platform=iphoneos; target=arm64-apple-ios16.0 ;;
    *) echo 'Choose cli, test, simulator or device.' >&2; exit 2 ;;
esac
caller=${3:?Supply the extracted private caller JSON}
audio=${4:?Supply a synthetic WAV sample}
app="$output/RamblerPoC.app"
mkdir -p "$app"
sdk=$(xcrun --sdk "$platform" --show-sdk-path)
xcrun --sdk "$platform" swiftc -sdk "$sdk" -target "$target" -parse-as-library -module-name RamblerPoC -O \
    "$root/Sources/RamblerCore.swift" "$root/Sources/OggOpus.swift" "$root/Sources/App.swift" \
    -o "$app/RamblerPoC"
cp "$caller" "$app/caller.json"
# The build directory and IPA stay private. Bundle resources must be readable
# by the app after iOS installs them under its own ownership.
chmod 644 "$app/caller.json"
cp "$audio" "$app/sample.wav"
python3 - "$app/Info.plist" "$platform" <<'PY'
import plistlib, sys
with open(sys.argv[1], 'wb') as f:
    plistlib.dump({
        'CFBundleIdentifier': 'com.kiwi.ramblerpoc',
        'CFBundleName': 'Rambler PoC', 'CFBundleDisplayName': 'Rambler PoC',
        'CFBundleExecutable': 'RamblerPoC', 'CFBundlePackageType': 'APPL',
        'CFBundleShortVersionString': '0.1.0', 'CFBundleVersion': '1',
        'MinimumOSVersion': '16.0', 'UIDeviceFamily': [1, 2],
        'CFBundleSupportedPlatforms': ['iPhoneSimulator' if sys.argv[2] == 'iphonesimulator' else 'iPhoneOS'],
        'NSMicrophoneUsageDescription': 'Record dictation for the Rambler proof of concept.',
        'UILaunchScreen': {}, 'UISupportedInterfaceOrientations': ['UIInterfaceOrientationPortrait'],
        'UIApplicationSceneManifest': {'UIApplicationSupportsMultipleScenes': False,
            'UISceneConfigurations': {'UIWindowSceneSessionRoleApplication': [{'UISceneConfigurationName': 'Default'}]}},
        'ITSAppUsesNonExemptEncryption': False,
    }, f)
PY
codesign --force --sign - "$app" >/dev/null
if [[ "$mode" == device ]]; then
    # Ad-hoc signatures establish bundle integrity. Kiwi must sign the IPA with
    # the user's development identity and profile before an iPhone can install it.
    mkdir -p "$output/Payload"
    ditto "$app" "$output/Payload/RamblerPoC.app"
    # AppleDouble resource-fork entries such as Payload/._RamblerPoC.app look
    # like a second app to IPA validators. Ship only the iOS bundle contents.
    ditto -c -k --norsrc --noextattr --noqtn --keepParent "$output/Payload" "$output/RamblerPoC.ipa"
fi
echo "$app"
