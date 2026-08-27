#!/usr/bin/env python
"""Fetch the source benchmark, pinned to the commit the paper used.

The thousand process shapes and the generator of the impact vectors come
from process-impact-benchmarks (Workneh, Sala, Rizzi, Cristani, Information
Systems 2025). This script clones that repository into this directory and
checks out the recorded commit, so the shapes underneath every stage are
the shapes of the paper and cannot drift under the seeds. Run it once;
running it again verifies the pin and touches nothing.

Usage
-----
    python setup.py
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent  # the experiment directory; everything generated lands here
BENCH = HERE / "process-impact-benchmarks"
URL = "https://github.com/PietroSala/process-impact-benchmarks.git"
COMMIT = "f591418b560a3c52ba5b5fada9274ea955d7c3f1"


def main() -> int:
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


if __name__ == "__main__":
    sys.exit(main())
