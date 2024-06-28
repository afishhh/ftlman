#!/usr/bin/env bash

set -euo pipefail

[[ -e ftl.dat ]] || {
	echo "ftl.dat doesn't exist, please copy your FTL data file here and rerun this script" >&2
	exit 1
}

[[ -e data-ftlman ]] || mkdir data-ftlman
[[ -e data-ftlman/ftl.dat ]] || ln ./ftl.dat data-ftlman

cargo run --release -- patch ./data-ftlman "Multiverse Assets.zip" "Multiverse Data.zip"
cargo run --release -- extract ./data-ftlman/ftl ./data-ftlman/ftl.dat
