#!/usr/bin/env bash
set -euo pipefail

tag=${1:?Usage: release-body.sh TAG}
assets="https://github.com/${GITHUB_REPOSITORY:?GITHUB_REPOSITORY must be set}/releases/download/$tag"

cat <<EOF
<img src="$assets/patch-icon.png" alt="Patchy app icon" width="96">

# Patch $tag

## Screenshots

| Desktop month | Desktop week | Desktop board |
| --- | --- | --- |
| <img src="$assets/desktop-pitch-month.png" alt="Desktop month" width="320"> | <img src="$assets/desktop-pitch-week.png" alt="Desktop week" width="320"> | <img src="$assets/desktop-pitch-board.png" alt="Desktop board" width="320"> |

| Phone | Floating panel | Linux pet bubble | Desktop tile widget |
| --- | --- | --- | --- |
| <img src="$assets/phone-pitch-month.png" alt="Phone month" width="240"> | <img src="$assets/overlay-pitch-panel.png" alt="Floating panel" width="240"> | <img src="$assets/overlay-pitch-bubble.png" alt="Linux pet bubble" width="96"> | <img src="$assets/desk-pitch.png" alt="Desktop tile widget" width="240"> |

EOF
cat "$(dirname "$0")/../release-notes.md"
