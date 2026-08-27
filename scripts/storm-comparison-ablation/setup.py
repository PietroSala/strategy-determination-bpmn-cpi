#!/usr/bin/env python
"""Fetch the source benchmark and unpack the recorded campaign.

Two preparations, both idempotent, so running this again verifies and
touches nothing. First, the thousand process shapes and the generator of
the impact vectors come from process-impact-benchmarks (Workneh, Sala,
Rizzi, Cristani, Information Systems 2025): this script clones that
repository into this directory and checks out the recorded commit, so the
shapes underneath every stage are the shapes of the paper and cannot
drift. Second, the campaign of the paper as it was recorded, instances,
optima, bounds, refinement rounds and results, ships with this repository
as a split compressed archive: this script reassembles it and unpacks it
into `default_experiment/`, the folder the stages read when they are run
with `--replay-experiment` and no value.

Usage
-----
    python setup.py
"""

from __future__ import annotations

import subprocess
import sys
import tarfile
from pathlib import Path

HERE = Path(__file__).resolve().parent  # the experiment directory; everything generated lands here
BENCH = HERE / "process-impact-benchmarks"
URL = "https://github.com/PietroSala/process-impact-benchmarks.git"
COMMIT = "f591418b560a3c52ba5b5fada9274ea955d7c3f1"

ARCHIVE = HERE / "default_experiment_archive"
DEFAULT = HERE / "default_experiment"


def fetch_benchmark() -> int:
    if not BENCH.exists():
        subprocess.run(["git", "clone", URL, str(BENCH)], check=True)
        subprocess.run(["git", "-C", str(BENCH), "checkout", "--detach", COMMIT],
                       check=True)
    head = subprocess.run(["git", "-C", str(BENCH), "rev-parse", "HEAD"],
                          capture_output=True, text=True, check=True).stdout.strip()
    if head != COMMIT:
        print(f"{BENCH} sits at {head[:12]} and the paper used {COMMIT[:12]};\n"
              f"move it aside and run this script again, or check out the pin:\n"
              f"    git -C {BENCH} checkout --detach {COMMIT}", file=sys.stderr)
        return 1
    print(f"process-impact-benchmarks at {COMMIT[:12]}, as the paper used it")
    return 0


def unpack_campaign() -> int:
    if DEFAULT.exists():
        print(f"default_experiment already unpacked at {DEFAULT}")
        return 0
    parts = sorted(ARCHIVE.glob("default_experiment.tar.xz.part-*"))
    if not parts:
        print(f"no archive under {ARCHIVE}; the recorded campaign is not in "
              "this checkout, and --replay-experiment with no value will not "
              "work until it is", file=sys.stderr)
        return 1
    whole = HERE / "default_experiment.tar.xz"
    with open(whole, "wb") as out:
        for part in parts:
            out.write(part.read_bytes())
    print(f"unpacking {len(parts)} parts, {whole.stat().st_size // 2**20} MB compressed")
    with tarfile.open(whole, "r:xz") as tar:
        tar.extractall(HERE)
    whole.unlink()
    if not DEFAULT.exists():
        print("the archive did not contain default_experiment", file=sys.stderr)
        return 1
    print(f"default_experiment unpacked at {DEFAULT}")
    return 0


def main() -> int:
    code = fetch_benchmark()
    return code if code else unpack_campaign()


if __name__ == "__main__":
    sys.exit(main())
