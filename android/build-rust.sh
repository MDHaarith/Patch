#!/usr/bin/env bash
# Builds libpatch_android.so for the APK's ABIs into app/src/main/jniLibs.
#
#   android/build-rust.sh [release|debug]     (default: release)
#
# Env: CARGO_TARGET_DIR (default <repo>/target-android, kept apart from the
# desktop build), ANDROID_NDK_HOME (default: the NDK from local.properties /
# $ANDROID_HOME/ndk/28.2.13676358), PATCH_ABIS (default "arm64-v8a x86_64").
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
profile="${1:-release}"

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$repo/target-android}"
sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
if [[ -z "$sdk" && -f "$here/local.properties" ]]; then
    sdk="$(sed -n 's/^sdk\.dir=//p' "$here/local.properties")"
fi
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$sdk/ndk/28.2.13676358}"
if [[ ! -d "$ANDROID_NDK_HOME" ]]; then
    echo "error: NDK not found at $ANDROID_NDK_HOME (set ANDROID_NDK_HOME)" >&2
    exit 1
fi

args=()
for abi in ${PATCH_ABIS:-arm64-v8a x86_64}; do
    args+=(-t "$abi")
done
release=()
[[ "$profile" == "release" ]] && release=(--release)

cd "$repo"
# -P 26: link against the minSdk's NDK libraries.
cargo ndk "${args[@]}" -P 26 -o "$here/app/src/main/jniLibs" build "${release[@]}" -p patch-android
