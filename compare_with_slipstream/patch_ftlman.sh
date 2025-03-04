#!/usr/bin/env bash

set -euo pipefail

[[ -e ftl.dat ]] || {
	echo "ftl.dat doesn't exist, please copy your FTL data file here and rerun this script" >&2
	exit 1
}

[[ -e data-ftlman ]] || mkdir data-ftlman
[[ -e data-ftlman/ftl.dat ]] || cp ./ftl.dat data-ftlman

cargo build --release
time ../target/release/ftlman patch -d ./data-ftlman "$@"
[[ -e data-ftlman/ftl ]] && rm -r ./data-ftlman/ftl
time ../target/release/ftlman extract ./data-ftlman/ftl ./data-ftlman/ftl.dat
cargo run --package=normalize_xml ./data-ftlman/ftl >/dev/null
