#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Julian Y. Richard Corbet
# SPDX-License-Identifier: FSL-1.1-ALv2
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
case ${RELEASE_COMPONENT:-all} in
  cargo|javascript|all) ;;
  *) printf 'RELEASE_COMPONENT must be cargo, javascript or all.\n' >&2; exit 2 ;;
esac
case ${1:-} in
  prepare|publish) ;;
  *) printf 'A prepare or publish stage is required.\n' >&2; exit 2 ;;
esac
case ${RELEASE_STAGE:-all} in
  all) ;;
  prepare|publish)
    if [[ $RELEASE_STAGE != "$1" ]]; then
      printf 'Stage %s was not selected; no work performed.\n' "$1"
      exit 0
    fi
    ;;
  *) printf 'RELEASE_STAGE must be all, prepare or publish.\n' >&2; exit 2 ;;
esac
exec python3 scripts/release.py "$1" "${RELEASE_COMPONENT:-all}"
