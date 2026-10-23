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
| <img src="$assets/desktop-matte-month.png" alt="Desktop month" width="320"> | <img src="$assets/desktop-matte-week.png" alt="Desktop week" width="320"> | <img src="$assets/desktop-matte-board.png" alt="Desktop board" width="320"> |

| Phone | Floating panel | Linux pet bubble | Desktop tile widget |
| --- | --- | --- | --- |
| <img src="$assets/phone-matte-month.png" alt="Phone month" width="240"> | <img src="$assets/overlay-matte-panel.png" alt="Floating panel" width="240"> | <img src="$assets/overlay-matte-bubble.png" alt="Linux pet bubble" width="96"> | <img src="$assets/desk-matte.png" alt="Desktop tile widget" width="240"> |

EOF
cat "$(dirname "$0")/../release-notes.md"
