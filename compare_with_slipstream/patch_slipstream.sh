#!/usr/bin/env bash

set -euo pipefail

[[ -e ftl.dat ]] || {
	echo "ftl.dat doesn't exist, please copy your FTL data file here and rerun this script" >&2
	exit 1
}

[[ -e data-slipstream ]] || mkdir data-slipstream
[[ -e data-slipstream/ftl.dat ]] || cp ./ftl.dat data-slipstream

bash ./slipstream/modman-cli.sh --patch "$@"
[ -n "${PATCHED_SLIPSTREAM_HASH:-}" ] &&
	sha256sum -c <<<"$PATCHED_SLIPSTREAM_HASH  data-slipstream/ftl.dat"
bash ./slipstream/modman-cli.sh --extract-dats="$PWD/data-slipstream/ftl"
cargo run --package=normalize_xml ./data-slipstream/ftl
