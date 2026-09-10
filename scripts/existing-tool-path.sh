#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Julian Y. Richard Corbet
# SPDX-License-Identifier: FSL-1.1-ALv2

# Expose existing tools without an installer, Nix realization or profile change.
ctypst_use_existing_lint_tools() {
  local tool candidate resolved
  local -a candidates
  if (($# == 0)); then set -- actionlint shellcheck reuse; fi
  for tool in "$@"; do
    if resolved=$(command -v "$tool"); then
      printf 'Existing lint tool: %s\n' "$resolved"
      continue
    fi
    case "$tool" in
      actionlint) candidates=(/nix/store/*-actionlint-*/bin/actionlint) ;;
      shellcheck) candidates=(/nix/store/*-shellcheck-*/bin/shellcheck) ;;
      reuse) candidates=(/nix/store/*-reuse-*/bin/reuse) ;;
      *) printf 'Unknown lint tool: %s.\n' "$tool" >&2; return 2 ;;
    esac
    resolved=
    for candidate in "${candidates[@]}"; do
      if [[ -x "$candidate" ]] && "$candidate" --version >/dev/null 2>&1; then
        resolved=$candidate
        break
      fi
    done
    if [[ -z "$resolved" ]]; then
      printf 'Missing provisioned lint tool: %s.\n' "$tool" >&2
      return 2
    fi
    export PATH="${resolved%/*}:$PATH"
    printf 'Existing lint tool: %s\n' "$resolved"
  done
}
