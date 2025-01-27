#!/usr/bin/env bash

set -euo pipefail

ROOT="$(dirname "$0")"
MODS_ROOT="$ROOT/mods"
SLIPSTREAM_ROOT="$ROOT/slipstream"

cd "$ROOT"

./download_slipstream.sh

[[ -e $MODS_ROOT ]] || mkdir "$MODS_ROOT"

testname="$1"
tested_mods_slipstream=()
tested_mods_ftlman=()

function add_mod() {
	ln -sf ../../mods/"$1".zip "$SLIPSTREAM_ROOT"/mods/
	tested_mods_slipstream+=("$1".zip)
	tested_mods_ftlman+=("$MODS_ROOT/$1".zip)
}

function _fetch_mod_googledrive() {
	[[ -e "$1" ]] ||
		"$ROOT"/google_drive_download.py -o "$@"
}

function _fetch_mod_wget() {
	[[ -e "$1" ]] ||
		wget -O "$@"
}

function _fetch_mod_local() { true; }

function zip_dir_to_file() {
	if hash 7z 2>/dev/null; then
		7z a -r "$2" "$1"/*
	else
		echo "No supported zip program is available" >&2
		exit 1
	fi
}

function _fetch_mod_ziplocal() {
	[[ -e "$1" ]] ||
		zip_dir_to_file "$2" "$1"
}

function download_mod() {
	fetcher="$1"
	name="$2"
	shift 2

	"_fetch_mod_$fetcher" "$MODS_ROOT/$name".zip "$@"
	add_mod "$name"
}

source modsets/"$testname.sh"

if [[ "$#" -gt 1 ]]; then
	declare -n args_name="tested_mods_$2"
	./patch_"$2".sh "${args_name[@]}"
else
	./patch_slipstream.sh "${tested_mods_slipstream[@]}"
	./patch_ftlman.sh "${tested_mods_ftlman[@]}"
fi
