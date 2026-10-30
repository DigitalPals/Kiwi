#!/bin/bash
set -euo pipefail
output=${1:?Usage: make-sample.sh /private/path/sample.wav}
scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT
/usr/bin/say -v Samantha -r 160 -o "$scratch/sample.aiff" \
    'Um, let us meet on Tuesday, actually no, Wednesday at two in the afternoon. Uh, please bring the budget report.'
/usr/bin/afconvert -f WAVE -d LEI16@16000 -c 1 "$scratch/sample.aiff" "$output"
