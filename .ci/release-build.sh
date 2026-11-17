#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Julian Y. Richard Corbet
# SPDX-License-Identifier: FSL-1.1-ALv2
#
# The files of a ctypst release, made by the release run (`ccid release build`): the crate and the
# Wasm npm package, prepared once by tools/release (which holds the custom Wasm build and its
# smoke test). The packages are the bytes every registry and copy carries.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
: "${CCID_RELEASE_DIR:?run by ccid release build}"
: "${CCID_RELEASE_VERSION:?run by ccid release build}"
commit=${CCID_RELEASE_COMMIT:-$(git rev-parse HEAD)}
# Tools the release tool requires and will not install: from the pinned nixpkgs of the verify job
# (substituted, never built), and the wasm-bindgen CLI at exactly the version Cargo.lock names.
tool_source=github:NixOS/nixpkgs/b7c2ada94fe99c15b0dbcf4d11fd7850b957a436
for path in $(nix build --no-link --print-out-paths "$tool_source#binaryen" "$tool_source#bun" "$tool_source#deno"); do
  PATH="$PATH:$path/bin"
done
bindgen=$(awk -F'"' '/^name = "wasm-bindgen"$/ {found=1; next} found && /^version = / {print $2; exit}' Cargo.lock)
if [ "$(wasm-bindgen --version 2>/dev/null | awk '{print $NF}')" != "$bindgen" ]; then
  cache_root=${CI_CACHE_ROOT:-${CARGO_HOME:-$HOME/.cache}}
  root="$cache_root/release-tools/wasm-bindgen-$bindgen"
  [ -x "$root/bin/wasm-bindgen" ] || cargo install --locked wasm-bindgen-cli --version "$bindgen" --root "$root"
  PATH="$root/bin:$PATH"
fi
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
export CI_COMMIT_SHA=$commit RELEASE_TAG="v$CCID_RELEASE_VERSION" RELEASE_ARTIFACT_DIR=$work
cargo run --quiet --locked --release --manifest-path tools/release/Cargo.toml --target-dir "${CARGO_TARGET_DIR:-$PWD/target}/release-tool" -- prepare all
version=$CCID_RELEASE_VERSION
cp "$work/ctypst-$version.crate" "$CCID_RELEASE_DIR/"
# One npm package: the archive of the javascript receipt (the JSR tarball is not a registry package).
npm=$(grep -o '"[^"]*\.tgz"' "$work/javascript.json" | head -1 | tr -d '"')
cp "$work/$npm" "$CCID_RELEASE_DIR/"
cp "$work/cargo.json" "$CCID_RELEASE_DIR/ctypst-$version-cargo-receipt.json"
cp "$work/javascript.json" "$CCID_RELEASE_DIR/ctypst-$version-javascript-receipt.json"
