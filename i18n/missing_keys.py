#!/usr/bin/env python3
import argparse
from pathlib import Path
from typing import cast

parser = argparse.ArgumentParser()
_ = parser.add_argument("file")
args = parser.parse_args()

file = Path(cast(str, args.file))
en_file = Path(__file__).parent / "en.ftl"


def extract_defined_keys(text: str) -> list[str]:
    keys: list[str] = []

    lines: list[str] = []
    for line in text.splitlines():
        comment_start = line.find("#")
        if comment_start != -1:
            line = line[:comment_start]
        line = line.strip()
        if line != "":
            lines.append(line)

    for line in lines:
        if (idx := line.find("=")) == -1:
            continue
        key = line[:idx].rstrip()
        keys.append(key)

    return keys


en_keys = extract_defined_keys(en_file.read_text())
keys = extract_defined_keys(file.read_text())

en_keys = set(en_keys)
keys = set(keys)

for missing in en_keys.difference(keys):
    print(f"Key '{missing}' is missing in {file.name} but present in {en_file.name}")

for extra in keys.difference(en_keys):
    print(f"Key '{extra}' is present in {file.name} but not in {en_file.name}")
