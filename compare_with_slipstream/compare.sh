#!/usr/bin/env bash

set -euo pipefail

if ! hash difft >/dev/null 2>&1; then
	echo "error: difftastic is not installed" >&2
	exit 1
fi

INFINITY=4294967296
export DFT_GRAPH_LIMIT=$INFINITY
export DFT_BYTE_LIMIT=$INFINITY
export DFT_PARSE_ERROR_LIMIT=$INFINITY
difft --check-only --ignore-comments --skip-unchanged data-slipstream/ftl data-ftlman/ftl | grep -P -o '^.*(?= ---)'
