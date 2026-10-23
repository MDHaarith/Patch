#!/usr/bin/env bash
set -euo pipefail

scripts=$(dirname "$0")
notes=$(cat "$scripts/../release-notes.md")
for tag in v0.0.0-preview nightly; do
  body=$(GITHUB_REPOSITORY=MDHaarith/Patch bash "$scripts/release-body.sh" "$tag")
  [[ "$body" == "<img src=\"https://github.com/MDHaarith/Patch/releases/download/$tag/patch-icon.png\""* ]]
  [[ "$body" == *"# Patch $tag"* ]]
  [[ $(printf '%s\n' "$body" | grep -o "https://github.com/MDHaarith/Patch/releases/download/$tag/" | wc -l) -eq 8 ]]
  [[ "$body" == *"$notes" ]]
done
echo "Release body checks passed (tagged and nightly)."
