#!/bin/bash
# Builds an iOS Simulator harness that uses stand-in classes instead of Gboard's. Install and open it in a simulator.
set -euo pipefail
root=$(cd -- "$(dirname -- "$0")" && pwd)
output=${1:?Usage: test-gboard.sh OUTPUT CALLER.json SAMPLE.wav}
caller=${2:?Caller configuration required}
sample=${3:?Synthetic audio required}
export DEVELOPER_DIR=${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}
"/bin/bash" "$root/test-speech.sh" "$output/speech-tests" "$sample"
"/bin/bash" "$root/build-gboard.sh" "$output" iphonesimulator
app="$output/GboardHarness.app"
mkdir -p "$app/Frameworks"
cp "$output/KiwiGboardRambler.dylib" "$app/Frameworks/"
cp "$caller" "$app/KiwiRamblerCaller.json"
cp "$sample" "$app/KiwiRamblerSample.wav"
python3 - "$app/Info.plist" <<'PY'
import plistlib, sys
with open(sys.argv[1], 'wb') as f:
    plistlib.dump(dict(CFBundleIdentifier='com.kiwi.gboardharness',
        CFBundleExecutable='GboardHarness', CFBundleName='Gboard Harness',
        CFBundlePackageType='APPL', CFBundleVersion='1', CFBundleShortVersionString='1.0',
        MinimumOSVersion='16.0', UIDeviceFamily=[1,2], UILaunchScreen={},
        UIApplicationSceneManifest=dict(UISceneConfigurations=dict(UIWindowSceneSessionRoleApplication=[
            dict(UISceneConfigurationName='Default', UISceneDelegateClassName='HarnessSceneDelegate')])),
        CFBundleURLTypes=[dict(CFBundleURLName='kiwi', CFBundleURLSchemes=['kiwi-harness'])],
        KiwiAppGroupAliases={'group.com.google.keyboard':'group.com.example.kiwi.keyboard',
            'group.com.google.common':'group.com.example.kiwi.common'}), f)
PY
sdk=$(xcrun --sdk iphonesimulator --show-sdk-path)
xcrun --sdk iphonesimulator clang -target arm64-apple-ios16.0-simulator -isysroot "$sdk" \
    -fobjc-arc -fmodules -framework UIKit -framework Foundation -framework QuartzCore \
    "$root/Tests/GboardHarness.m" -o "$app/GboardHarness"
codesign --force --deep --sign - "$app" >/dev/null
