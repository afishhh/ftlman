#!/usr/bin/env bash

set -euo pipefail

if ! hash difft >/dev/null 2>&1; then
	if hash nix >/dev/null 2>&1; then
		exec nix shell nixpkgs#difftastic -c "$0" "$@"
	else
		echo "error: difftastic is not installed" >&2
		exit 1
	fi
fi

INFINITY=1073741824
export DFT_GRAPH_LIMIT=$INFINITY
export DFT_BYTE_LIMIT=$INFINITY
export DFT_PARSE_ERROR_LIMIT=$INFINITY
difft --check-only --ignore-comments --skip-unchanged data-slipstream/ftl data-ftlman/ftl | grep -P -o '^.*(?= ---)'
