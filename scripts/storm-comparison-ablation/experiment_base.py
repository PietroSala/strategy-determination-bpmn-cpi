"""Where the data of a stage lives: this directory, or a recorded experiment.

Every stage script computes its data directories from one base. Without
flags the base is the directory of the scripts, and the stages build the
experiment there from nothing. With `--replay-experiment` the base is a
recorded experiment instead: `--replay-experiment <dir>` names a folder
structured like this one (`bpmn-cpi-benchmarks/`, `benchmarks-optima/`,
`benchmarks-refinements/`, and so on), and `--replay-experiment` alone
names `default_experiment/`, the campaign of the paper as it was recorded,
unpacked beside the scripts by `setup.py`. The flag is read and removed
here, before a script parses its own arguments, and the answer is cached
so every module of one run agrees on the base.
"""

from __future__ import annotations

import sys
from pathlib import Path

_cached: Path | None = None


def base(here: Path) -> Path:
    global _cached
    if _cached is None:
        _cached = _compute(here)
    return _cached


def _compute(here: Path) -> Path:
    argv = sys.argv
    if "--replay-experiment" not in argv:
        return here
    i = argv.index("--replay-experiment")
    has_value = i + 1 < len(argv) and not argv[i + 1].startswith("--")
    target = Path(argv[i + 1]).resolve() if has_value else here / "default_experiment"
    end = i + 2 if has_value else i + 1
    del argv[i:end]
    if not target.exists():
        raise SystemExit(
            f"{target} does not exist"
            + ("" if has_value else "; run setup.py to unpack the shipped campaign")
        )
    return target
