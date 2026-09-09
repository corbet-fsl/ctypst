#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Julian Y. Richard Corbet
# SPDX-License-Identifier: FSL-1.1-ALv2
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
ci_root=$PWD
case ${CI_LINKER:-system} in
  system) ;;
  mold)
    ci_mold=$(command -v mold)
    # Nix's linker wrapper prepends flags; -run must reach mold first.
    ci_mold_origin=$(dirname "$(realpath "$ci_mold")")/../nix-support/orig-bintools
    if [[ -f $ci_mold_origin ]]; then
      ci_mold="$(<"$ci_mold_origin")/bin/mold"
    fi
    "$ci_mold" --version
    # Intercept linker execution without invalidating every Cargo fingerprint.
    CI_LINKER=system exec "$ci_mold" -run bash "$ci_root/scripts/ci.sh" "$@"
    ;;
  *)
    printf 'CI_LINKER must be system or mold.\n' >&2
    exit 2
    ;;
esac
ci_toolchain=${CI_RUST_TOOLCHAIN:-1.92.0}
ci_cargo=(cargo)
ci_rustc=(rustc)
if [[ $ci_toolchain != system ]]; then
  ci_cargo+=("+$ci_toolchain")
  ci_rustc+=("+$ci_toolchain")
fi

rust_quality() {
  ci_package_files=$("${ci_cargo[@]}" package --locked --list)
  if printf '%s\n' "$ci_package_files" | grep -E '(^|/)(node_modules|\.venv|__pycache__)/' >/dev/null; then
    printf 'Generated dependencies would enter the crate; use a clean source checkout.\n' >&2
    return 1
  fi
  "${ci_rustc[@]}" --version
  "${ci_cargo[@]}" fmt --check
  "${ci_cargo[@]}" clippy --locked --all-targets --all-features -- -D warnings
  "${ci_cargo[@]}" check --locked --no-default-features
  for ci_feature in document-fonts format measure pdf raster svg wasm; do
    "${ci_cargo[@]}" check --locked --no-default-features --features "$ci_feature"
  done
  "${ci_cargo[@]}" publish --locked --dry-run
}

rust_tests() {
  "${ci_rustc[@]}" --version
  "${ci_cargo[@]}" test --locked --all-features
}

python_checks() (
  cd "$ci_root/bindings/python"
  "${ci_cargo[@]}" fmt --check
  "${ci_cargo[@]}" clippy --locked --all-targets -- -D warnings
  ci_python=${CI_PYTHON:-python3}
  ci_venv=$(mktemp -d "${TMPDIR:-/tmp}/ctypst-python.XXXXXX")
  trap 'rm -rf -- "$ci_venv"' EXIT
  uv venv --python "$ci_python" "$ci_venv"
  uv pip install --python "$ci_venv/bin/python" 'maturin==1.15.0' 'pytest==9.1.1'
  source "$ci_venv/bin/activate"
  python --version
  if [[ $ci_toolchain != system ]]; then
    export RUSTUP_TOOLCHAIN=$ci_toolchain
  fi
  maturin develop --locked
  pytest tests/ -q
)

javascript_checks() (
  # Keep generated assets and installed dependencies out of the source package.
  ci_js_workspace=$(mktemp -d "${TMPDIR:-/tmp}/ctypst-javascript.XXXXXX")
  trap 'rm -rf -- "$ci_js_workspace"' EXIT
  tar --exclude=.git --exclude=target --exclude=node_modules --exclude=.venv \
    --exclude=dist --exclude='*.tgz' --exclude="${ci_js_workspace##*/}" \
    -cf - . | tar -C "$ci_js_workspace" -xf -
  ci_root=$ci_js_workspace
  cd "$ci_root/js/@corbet-labs/ctypst"
  bun --version
  bun install --frozen-lockfile
  python3 - "$ci_root" <<'PY'
import json
from pathlib import Path
import sys
import tomllib

root = Path(sys.argv[1])
version = tomllib.loads((root / "Cargo.toml").read_text())["package"]["version"]
for path in ["js/@corbet-labs/ctypst/package.json", "js/@corbet-labs/ctypst/jsr.json"]:
    if json.loads((root / path).read_text())["version"] != version:
        raise SystemExit(f"{path} does not match crate version {version}")
for path, table in [("bindings/python/Cargo.toml", "package"), ("bindings/python/pyproject.toml", "project")]:
    if tomllib.loads((root / path).read_text())[table]["version"] != version:
        raise SystemExit(f"{path} does not match crate version {version}")
print(f"All package versions match {version}")
PY
  bash scripts/sync-assets.sh
  bun ./node_modules/typescript/bin/tsc --noEmit -p tsconfig.json
  bun scripts/conformance.mts
  ci_pack_dir=$ci_js_workspace/packed
  mkdir "$ci_pack_dir"
  bun pm pack --destination "$ci_pack_dir" >/dev/null
  test "$(tar tzf "$ci_pack_dir"/*.tgz | grep -c '\.ttf$')" = 16
)

license_checks() {
  uvx --from 'reuse[charset-normalizer]==6.0.0' reuse lint
}

case ${1:-all} in
  quality) rust_quality ;;
  test) rust_tests ;;
  python) python_checks ;;
  javascript) javascript_checks ;;
  license) license_checks ;;
  all) license_checks; rust_quality; rust_tests; python_checks; javascript_checks ;;
  *) printf 'usage: bash scripts/ci.sh [all|quality|test|python|javascript|license]\n' >&2; exit 2 ;;
esac
