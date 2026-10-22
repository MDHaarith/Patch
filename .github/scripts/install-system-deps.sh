#!/usr/bin/env bash
set -euo pipefail

if [[ $# -eq 0 ]]; then
  echo "Usage: $0 PACKAGE..." >&2
  exit 2
fi

# GitHub's Ubuntu runners use this mirror list. Prefer the official archive
# over the Azure mirror, which has stalled downloads in this workflow.
printf '%s\n' \
  $'https://archive.ubuntu.com/ubuntu/\tpriority:1' \
  $'https://security.ubuntu.com/ubuntu/\tpriority:2' \
  $'http://azure.archive.ubuntu.com/ubuntu/\tpriority:3' \
  | sudo tee /etc/apt/apt-mirrors.txt > /dev/null

apt_options=(
  -o Acquire::Retries=2
  -o Acquire::http::Timeout=20
  -o Acquire::https::Timeout=20
)

# Workflow step limits also stop mirrors that trickle data without going idle.
sudo env DEBIAN_FRONTEND=noninteractive apt-get "${apt_options[@]}" update
sudo env DEBIAN_FRONTEND=noninteractive apt-get "${apt_options[@]}" \
  install -y --no-install-recommends "$@"
