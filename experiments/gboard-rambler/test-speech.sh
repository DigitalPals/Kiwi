#!/bin/bash
set -euo pipefail
root=$(cd -- "$(dirname -- "$0")" && pwd)
output=${1:?Usage: test-speech.sh OUTPUT SAMPLE.wav [CALLER.json]}
sample=${2:?Synthetic speech fixture required}
export DEVELOPER_DIR=${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}
mkdir -p "$output"
xcrun swiftc -parse-as-library -O "$root/Sources/RamblerCore.swift" "$root/Sources/OggOpus.swift" \
    "$root/Gboard/SpeechActivity.swift" "$root/Tests/SpeechActivityTests.swift" -o "$output/speech-tests"
"$output/speech-tests" "$sample" "${@:3}"
