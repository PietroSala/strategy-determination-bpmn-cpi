#!/usr/bin/env python
"""The box the refinement game plays in, taken from the exact optima.

Every round of the game holds the components it is not bisecting at their
current upper bound, which is meant to be a constraint no policy can fail. Taken
from the decimals Storm prints, rounded to about ten significant digits, it is
not: at the printed maximum the constraint binds by roughly 1e-10, and the two
checkers resolve that hair differently, which is where 1336 of the first 60 940
replayed rounds disagreed. Both readings were right about the question they were
actually asked, and the question was the wrong one.

So the box is rebuilt here from `sdcpi optima`, which computes the least
and the greatest expected impact of every component by one traversal of the
tree. Two things are gained.

  Coverage. The optima exist for all 54 000 instances of the grid, including the
  14 630 whose state space no model checker on this machine can build, so the
  game is no longer confined to the instances Storm could bound.

  A box that is a box. The upper bounds are padded outward by a relative 1e-6
  before they are written. The optima here are computed in floating point while
  Storm compares rationally, so the two cannot be made to agree on the last bit;
  the padding is a million times larger than any difference we have measured
  between them, and a thousand times smaller than what ten bisections
  distinguish, so the edge of the box stops binding and the game is unchanged.

  The padding works because what it pads is exact. `optima` prints the shortest
  form that reads back as the same double, so nothing is rounded on the way
  here. It used to print ten decimal places, which keeps four significant
  digits of a component worth 2e-7 and loses 2.8e-12 of it, against a relative
  padding worth 2.0e-13: the padding was then two hundred times smaller than
  the rounding it was meant to absorb, and a constraint that could not bind
  bound. Every round zero that answered no, twenty eight of them, was that.

The result mirrors the layout of the instances, and is written in the same shape
as `benchmarks-bounds` so that everything downstream reads either:

    bpmn-cpi-benchmarks/3-nested/2-independent/1-process_number/4-random.yaml
    benchmarks-optima/3-nested/2-independent/1-process_number/4-random.yaml

Usage
-----
    python exact_optima.py --all --jobs 8
    python exact_optima.py 3-2-1-4-random
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import yaml

HERE = Path(__file__).resolve().parent.parent  # the repository root
INSTANCES = HERE / "bpmn-cpi-benchmarks"
OPTIMA = HERE / "benchmarks-optima"
PACO = HERE / "target" / "release" / "sdcpi"

LINE = re.compile(r"^component\s+(\d+)\s+min\s+([\d.eE+-]+)\s+max\s+([\d.eE+-]+)",
                  re.M)
SECONDS = re.compile(r"^seconds\s+([\d.eE+-]+)", re.M)

# the edge of the box must bind on nobody, and the two tools compare in
# different arithmetic, so the upper bounds are opened by this much
PAD = 1e-6


def rel_to_key(rel: Path) -> str:
    p = rel.parts
    return f"{p[0].split('-')[0]}-{p[1].split('-')[0]}-{p[2].split('-')[0]}-{rel.stem}"


def one(rel: Path, force: bool) -> str:
    out = (OPTIMA / rel).with_suffix(".yaml")
    if out.exists() and not force:
        return "skip"
    src = (INSTANCES / rel).with_suffix(".yaml")
    started = time.time()
    r = subprocess.run([str(PACO), "optima", str(src)],
                       capture_output=True, text=True)
    elapsed = time.time() - started
    rows = LINE.findall(r.stdout)
    meta = yaml.safe_load(src.read_text())
    out.parent.mkdir(parents=True, exist_ok=True)
    lines = [f"# {rel_to_key(rel)}",
             f"instance: {rel_to_key(rel)}",
             f"dimensions: {meta['dimensions']}",
             f"mode: {meta['mode']}",
             f"tasks: {meta['tasks']}",
             f"nodes: {meta['nodes']}",
             "tool: sdcpi optima",
             f"padding: {PAD:g}",
             f"seconds: {round(elapsed, 4)}"]
    if len(rows) != meta["dimensions"]:
        first = next((l for l in (r.stdout + r.stderr).splitlines() if l.strip()),
                     "no optima returned")
        lines.append(f'failed: "{first.strip()[:200]}"')
        out.write_text("\n".join(lines) + "\n")
        return "fail"
    lines.append("bounds:")
    for j, lo, hi in rows:
        # written to seventeen significant digits, which is every bit a double
        # holds, and the maximum opened by the padding above
        lines += [f"  - component: {int(j)}",
                  f"    min: {float(lo):.17g}",
                  f"    max: {float(hi) * (1 + PAD):.17g}",
                  f"    max_raw: {float(hi):.17g}"]
    out.write_text("\n".join(lines) + "\n")
    return "ok"


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("keys", nargs="*")
    p.add_argument("--all", action="store_true")
    p.add_argument("--force", action="store_true")
    p.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) - 4))
    a = p.parse_args()

    if a.all:
        rels = sorted(q.relative_to(INSTANCES).with_suffix("")
                      for q in INSTANCES.rglob("*.yaml"))
    else:
        rels = []
        for k in a.keys:
            n, i, num, rest = k.split("-", 3)
            rels.append(Path(f"{n}-nested") / f"{i}-independent"
                        / f"{num}-process_number" / rest)
    if not rels:
        p.error("give some keys or --all")

    done: dict[str, int] = {}
    started = time.time()
    with ThreadPoolExecutor(max_workers=a.jobs) as pool:
        for n, outcome in enumerate(pool.map(lambda r: one(r, a.force), rels), 1):
            done[outcome] = done.get(outcome, 0) + 1
            if outcome == "fail" or n % 5000 == 0 or n == len(rels):
                print(f"[{n}/{len(rels)}] {done}  {round(time.time() - started)} s",
                      flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
