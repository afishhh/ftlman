#!/usr/bin/env python3

# NOTE:
# NOTE: This file is taken from https://github.com/afishhh/pkgs (fetchgdrive fetcher)
# NOTE:

import re
import sys
from typing import IO
import requests
from math import floor
from time import monotonic
import argparse

parser = argparse.ArgumentParser()
parser.add_argument("--output", "-o", type=argparse.FileType('wb+'), required=False)
parser.add_argument("file_id", help="Google drive file ID")
args = parser.parse_args()

def get_filename(response: requests.Response) -> str | None:
    try:
        cd = response.headers["content-disposition"]
        cd = cd[cd.find("filename=") + 10 :]
        cd = cd[: cd.find('"')]
    except IndexError:
        return
    except KeyError:
        return
    return cd


def print_status_text(done: int, total: int):
    print(f"{done}/{total} bytes dowloaded ({done / total * 100:.1f}%)", file=sys.stderr)


def update_progress_bar(done: int, total: int):
    WIDTH = 30
    sys.stderr.write("\r[")
    done_cells = floor(WIDTH * (done / total))
    sys.stderr.write("#" * done_cells)
    sys.stderr.write(" " * (WIDTH - done_cells))
    sys.stderr.write(f"] {done / total * 100:.1f}%")
    sys.stderr.flush()


def finish_progress_bar():
    update_progress_bar(1, 1)
    print()


def stream_with_progress(response: requests.Response, output: IO):
    total = int(response.headers.get("content-length", 0)) or None
    if total is None:
        print("Content-Length is not present", file=sys.stderr)

    if sys.stderr.isatty():
        progress_update = update_progress_bar
        progress_finish = finish_progress_bar
    else:
        progress_update = print_status_text
        progress_finish = lambda: None

    last_progress_update = monotonic()
    done = 0
    if total is not None:
        progress_update(done, total)
    # `response.iter_content(chunk_size=None)` cannot be used because of
    # requests bug #5536 (https://github.com/psf/requests/issues/5536)
    for chunk in response.raw.stream():
        output.write(chunk)

        done += len(chunk)
        if total is not None:
            now = monotonic()
            if now - last_progress_update > 0.5:
                progress_update(done, total)
                last_progress_update = now
    if total is not None:
        progress_finish()


def open_output_file(response: requests.Response) -> IO:
    if args.output:
        return args.output
    filename = get_filename(response) or "output"
    print(filename)
    return open(filename, "wb+")


file_id = args.file_id

download_url = f"https://drive.google.com/uc?export=download&id={file_id}"
print("Fetching initial response", file=sys.stderr)
response = requests.get(download_url, stream=True)

if (
    response.headers["content-type"].startswith("text/html")
    and file_id in response.text
):
    UUID_REGEX = re.compile(
        r'"([0-9a-z]{8}-[0-9a-z]{4}-[0-9a-z]{4}-[0-9a-z]{4}-[0-9a-z]{12})"'
    )
    assert (match := UUID_REGEX.search(response.text))
    (uuid,) = match.groups()

    print("Fetching confirmed response", file=sys.stderr)
    response = requests.get(
        "https://drive.usercontent.google.com/download",
        params={"id": file_id, "export": "download", "confirm": "t", "uuid": uuid},
        stream=True,
    )
else:
    print("Initial response is not a virus check", file=sys.stderr)

stream_with_progress(response, open_output_file(response))
