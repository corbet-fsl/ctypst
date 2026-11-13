#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Julian Y. Richard Corbet
# SPDX-License-Identifier: FSL-1.1-ALv2
#
# The files of a ctypst release, made by the release run (`ccid release build`): the crate and the
# Wasm npm package, prepared once by scripts/release.py (which holds the custom Wasm build and its
# smoke test). The packages are the bytes every registry and copy carries.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
: "${CCID_RELEASE_DIR:?run by ccid release build}"
: "${CCID_RELEASE_VERSION:?run by ccid release build}"
commit=${CCID_RELEASE_COMMIT:-$(git rev-parse HEAD)}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
export CI_COMMIT_SHA=$commit RELEASE_TAG="v$CCID_RELEASE_VERSION" RELEASE_ARTIFACT_DIR=$work
python3 scripts/release.py prepare all
version=$CCID_RELEASE_VERSION
cp "$work/ctypst-$version.crate" "$CCID_RELEASE_DIR/"
# One npm package: the archive of the javascript receipt (the JSR tarball is not a registry package).
npm=$(python3 -c 'import json,sys; print(next(n for n in json.load(open(sys.argv[1]))["artifacts"] if n.endswith(".tgz")))' "$work/javascript.json")
cp "$work/$npm" "$CCID_RELEASE_DIR/"
cp "$work/cargo.json" "$CCID_RELEASE_DIR/ctypst-$version-cargo-receipt.json"
cp "$work/javascript.json" "$CCID_RELEASE_DIR/ctypst-$version-javascript-receipt.json"
