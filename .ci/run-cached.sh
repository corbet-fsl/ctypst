#!/usr/bin/env bash
# Execute the manifest's selected checks with content-keyed result reuse.
set -euo pipefail
exec 2>&1
: "${CCID_BIN:?Missing verified ccid job runtime}"
: "${CI_COMMIT_SHA:?Missing verified source revision}"
: "${CI_REPOSITORY_URL:?Missing canonical repository identity}"
: "${CHECKS:?Missing declared check selection}"
if [ "$CHECKS" != "quality,test,python,javascript,license" ]; then
  echo "CHECKS must be exactly 'quality,test,python,javascript,license', got: $CHECKS" >&2
  exit 2
fi
cache_root=${CI_CACHE_ROOT:-${CARGO_HOME:-}}
[[ $cache_root = /* ]] || { echo 'An absolute persistent CI_CACHE_ROOT or CARGO_HOME is required' >&2; exit 2; }
namespace=$(printf '%s' "$CI_REPOSITORY_URL" | sha256sum | cut -c1-24)
state="$cache_root/ccid-cached/$namespace"
toolchains=${CI_TOOLCHAIN_ROOT:-$cache_root/ccid-cached/toolchains}
[[ $toolchains = /* ]] || { echo 'CI_TOOLCHAIN_ROOT must be absolute' >&2; exit 2; }
mkdir -p "$state" "$toolchains"
exec 9>"$state/lock"
flock -w "${CI_TIMEOUT:-2700}" 9
mkdir -p "$state/moon-cache/hashes" "$state/moon-cache/outputs" "$state/moon-home"
mkdir -p .moon/cache
ln -s "$state/moon-cache/hashes" .moon/cache/hashes
ln -s "$state/moon-cache/outputs" .moon/cache/outputs
export RUNNER_TEMP="${RUNNER_TEMP:-$(mktemp -d)}"
# Tool installation is an explicit part of this job, in persistent CI storage.
# Pinned nixpkgs b7c2ada (narHash-pinned): bun 1.3.13 matches
# ci.yml oven-sh/setup-bun bun-version 1.3.13 exactly; python313 3.13.14
# matches ci.yml setup-python 3.13 major; reuse-action is
# fsfe/reuse-action@676e2d5... (v6.0.0) whose Dockerfile is FROM fsfe/reuse:6
# (floating major-6, not a package pin) — runner supplies nix reuse 6.2.0
# (pinned 6.x) and probes `reuse --version`, the `reuse lint` gate itself is
# the requirement; actionlint/shellcheck provide release-guards lint tools
# (verify itself uses reuse only, guards stay separate).
# uv is NOT taken from nix (nixpkgs b7c2ada uv is 0.12.1, CI pins uv==0.12.5
# via `python -m pip install 'uv==0.12.5'`); the exact CI pin is enforced
# job-local below with hash, never unpinned. PyPI uv 0.12.5 manylinux x86_64
# (https://files.pythonhosted.org/packages/93/22/dacc9a0bc8604187a1ba954a3aef8329e4104eb0af772d2c3c634893bd9b/uv-0.12.5-py3-none-manylinux_2_17_x86_64.manylinux2014_x86_64.whl)
# sha256 3e195ccf1ed60c8bb24a6447ce306441a4181d54b602407e09bc56e963911c15.
tool_source=github:NixOS/nixpkgs/b7c2ada94fe99c15b0dbcf4d11fd7850b957a436?narHash=sha256-IItrdb7Puk05RqOBWZYFC5X6Wl1sJmCfh5MWVHw5iMM=
tool_paths=$(nix build --no-link --print-out-paths "$tool_source#moon" "$tool_source#proto" "$tool_source#bun" "$tool_source#reuse" "$tool_source#actionlint" "$tool_source#shellcheck")
for path in $tool_paths; do PATH="$path/bin:$PATH"; done
# Python 3.13 (matches ci.yml setup-python 3.13 major; nix 3.13.14) with pip
# so `uv venv`/`uv pip` and the CI `pip install uv==0.12.5` work job-locally
# with no system install. Same nixpkgs revision+NAR.
python_env=$(nix build --no-link --print-out-paths --impure --expr '(builtins.getFlake "'"$tool_source"'").legacyPackages.${builtins.currentSystem}.python313.withPackages (p: [ p.pip ])')
test -n "$python_env" || { echo 'nix python313 env resolution failed' >&2; exit 2; }
PATH="$python_env/bin:$PATH"
for path in $tool_paths; do PATH="$path/bin:$PATH"; done
export PROTO_HOME="$toolchains/proto" RUSTUP_HOME="$toolchains/rustup"
shared_cargo_home=${CARGO_HOME:-}
export CARGO_HOME="$toolchains/cargo" MOON_HOME="$state/moon-home"
export PROTO_AUTO_CLEAN=false PROTO_TELEMETRY=false PROTO_YES=true
export RUSTUP_INIT_SKIP_PATH_CHECK=yes RUSTUP_TOOLCHAIN=stable
export PATH="$(dirname "$CCID_BIN"):$CARGO_HOME/bin:$PROTO_HOME/shims:$PROTO_HOME/bin:$PATH"
export CI_JOBS="${CI_JOBS:-2}" CARGO_BUILD_JOBS="${CI_JOBS:-2}"
export RUST_TEST_THREADS="${CI_TEST_THREADS:-2}"
export CARGO_TERM_COLOR="${CARGO_TERM_COLOR:-always}"
export RUSTUP_TOOLCHAIN=stable
# Shared-first toolchain lock: jobs share a provisioned toolchain and take
# exclusive only to provision missing pieces. Same lock file, so old
# whole-job-exclusive runners interoperate (they just serialize).
TOOLCHAIN_LOCK_FILE="$toolchains/install.lock"
TOOLCHAIN_LOCK_TIMEOUT="${CI_TIMEOUT:-2700}"
TOOLCHAIN_WANT_NIGHTLY=0
TOOLCHAIN_WASM_TARGETS=
# shellcheck source=toolchain-lock.sh
source "$(dirname "${BASH_SOURCE[0]}")/toolchain-lock.sh"
toolchain_ensure
cargo_bin=$("$CARGO_HOME/bin/rustup" which --toolchain stable cargo)
test -x "$cargo_bin"
export PATH="$(dirname "$cargo_bin"):$PATH"
export CCID_SOURCE_REVISION="$CI_COMMIT_SHA"
echo 'ctypst:stage=render'
"$CCID_BIN" render --repo "$PWD" --check
echo 'ctypst:stage=tool-versions'
proto --version
moon --version
rustc -vV
cargo --version
bun --version
python3 --version
echo 'ctypst:stage=python-env'
# The Crow Python env must provide pip (for the uv pin below) and venv (for
# `scripts/ci.sh python`'s `uv venv`); both are job-local, no system install.
python3 -m pip --version
python3 -m venv --help >/dev/null
# Exact CI pin `uv==0.12.5` with hash enforcement: job-local
# --require-hashes lock; an unknown wheel is never accepted.
# Job-owned venv under job temp (never worker HOME, never persistent state):
# a fresh venv carries no EXTERNALLY-MANAGED marker, so the pinned install
# works without system pip, relaxed hashes, or HOME credentials.
UV_VENV="$RUNNER_TEMP/uv-venv"
"$(command -v python3)" -m venv "$UV_VENV"
export PATH="$UV_VENV/bin:$PATH"
uv_req="$RUNNER_TEMP/uv-pin.txt"
printf 'uv==0.12.5 --hash=sha256:3e195ccf1ed60c8bb24a6447ce306441a4181d54b602407e09bc56e963911c15\n' > "$uv_req"
echo 'ctypst:stage=uv-pin'
# Silent-exit diagnosis (208/3 died here with no output): label each bounded
# step and capture its output explicitly instead of relying on ambient log
# capture. No tracing, no environment dumps; status, pin, and gates unchanged.
# Pin hash verified against PyPI uv-0.12.5 manylinux_x86_64 metadata
# (sha256 3e195ccf..., not yanked).
echo 'ctypst:uv-pin=detect'
uv_version_raw=$(uv --version 2>&1 || true)
uv_version="$(printf '%s' "$uv_version_raw" | awk '{print $1" "$2}')"
echo "ctypst:uv-pin-detected version='$uv_version'"
if [ "$uv_version" != "uv 0.12.5" ]; then
  echo 'ctypst:uv-pin=install'
  pip_out=$("$UV_VENV/bin/python" -m pip install --require-hashes -r "$uv_req" 2>&1) || { rc=$?; printf '%s\n' "$pip_out"; echo "ctypst:uv-pin-install-failed rc=$rc prior-version='$uv_version'" >&2; exit 1; }
  printf '%s\n' "$pip_out"
fi
echo 'ctypst:uv-pin=assert'
uv_version_raw=$(uv --version 2>&1 || true)
uv_version="$(printf '%s' "$uv_version_raw" | awk '{print $1" "$2}')"
echo "ctypst:uv-pin-asserted version='$uv_version'"
[ "$uv_version" = "uv 0.12.5" ] || { echo "CI requires uv exactly 0.12.5, got: $uv_version" >&2; exit 2; }
reuse --version
actionlint --version
shellcheck --version
echo 'ctypst:stage=cached-checks'
"$CCID_BIN" cached --repo "$PWD" --check "$CHECKS"
IFS=, read -r -a selected_checks <<< "$CHECKS"
for check in "${selected_checks[@]}"; do test -s ".ccid/results/$check.jsonl"; done
