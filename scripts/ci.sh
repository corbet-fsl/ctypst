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
# Use the caller's installed compiler. GitHub Actions selects current stable
# with RUSTUP_TOOLCHAIN; Crow supplies its already installed worker tools.
rust_version() {
  rustc --version --verbose
  cargo --version
}

rust_quality() {
  rust_version
  ci_package_files=$(cargo package --locked --list)
  if printf '%s\n' "$ci_package_files" | grep -E '(^|/)(node_modules|\.venv|__pycache__)/' >/dev/null; then
    printf 'Generated dependencies would enter the crate; use a clean source checkout.\n' >&2
    return 1
  fi
  cargo fmt --check
  cargo fmt --check --manifest-path "$ci_root/tools/release/Cargo.toml"
  cargo clippy --locked --all-targets --all-features -- -D warnings
  cargo clippy --locked --manifest-path "$ci_root/tools/release/Cargo.toml" --target-dir "${CARGO_TARGET_DIR:-$ci_root/target}/release-tool" --all-targets -- -D warnings
  cargo check --locked --no-default-features
  for ci_feature in conformance document-fonts format measure pdf raster svg wasm; do
    cargo check --locked --no-default-features --features "$ci_feature"
  done
  cargo publish --locked --dry-run
}

rust_tests() {
  rust_version
  cargo test --locked --all-features
  cargo test --locked --manifest-path "$ci_root/tools/release/Cargo.toml" --target-dir "${CARGO_TARGET_DIR:-$ci_root/target}/release-tool"
}

python_checks() (
  cd "$ci_root/bindings/python"
  rust_version
  cargo fmt --check
  cargo clippy --locked --all-targets -- -D warnings
  ci_python=${CI_PYTHON:-python3}
  ci_venv=$(mktemp -d "${TMPDIR:-/tmp}/ctypst-python.XXXXXX")
  trap 'rm -rf -- "$ci_venv"' EXIT
  uv venv --python "$ci_python" "$ci_venv"
  uv pip install --python "$ci_venv/bin/python" 'maturin==1.15.0' 'pytest==9.1.1'
  # shellcheck source=/dev/null # The virtual environment creates this file.
  source "$ci_venv/bin/activate"
  python --version
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
  ci_js_root=$ci_js_workspace
  cd "$ci_js_root/js/@corbet-labs/ctypst"
  bun --version
  bun install --frozen-lockfile
  cargo run --quiet --locked --manifest-path "$ci_root/tools/release/Cargo.toml" --target-dir "${CARGO_TARGET_DIR:-$ci_root/target}/release-tool" -- versions "$ci_js_root"
  bash scripts/sync-assets.sh
  bun ./node_modules/typescript/bin/tsc --noEmit -p tsconfig.json
  bun scripts/conformance.mts
  ci_pack_dir=$ci_js_workspace/packed
  mkdir "$ci_pack_dir"
  bun pm pack --destination "$ci_pack_dir" >/dev/null
  test "$(tar tzf "$ci_pack_dir"/*.tgz | grep -c '\.ttf$')" = 16
)

license_checks() {
  # shellcheck source=scripts/existing-tool-path.sh
  source "$ci_root/scripts/existing-tool-path.sh"
  ctypst_use_existing_lint_tools reuse
  reuse lint
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
