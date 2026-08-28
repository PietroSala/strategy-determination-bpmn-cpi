#!/usr/bin/env python
"""Export the recorded rounds as one flat table, one row per run.

A run is one question of the refinement game on one instance: every row
carries the identity of the question, the one answer, and, for each
configuration that played it, the seconds it took, together with the size
of the strategy the ablated configurations record. Absent is empty: a
configuration that did not play the question, or ran out of time on it,
leaves its cells blank.

The columns:

    instance, nested, independent, process_number, dimensions, mode,
    diagonal, round, component        the identity of the question;
    response                          yes or no, the one answer, checked
                                      to be the same in every file that
                                      answered it, the export failing on
                                      any disagreement;
    seconds_full, seconds_accept,     the wall seconds of the search with
    seconds_reject, seconds_none      both tests, the accepting test
                                      alone, the rejecting test alone,
                                      and neither;
    seconds_storm                     the wall seconds of Storm;
    histories_X, open_X, decisions_X  the size of the returned strategy,
                                      recorded by the three ablated
                                      configurations (X in accept,
                                      reject, none) on a positive answer:
                                      the histories of the winning
                                      frontier, how many are open, and
                                      the decisions prescribed.

Usage
-----
    python3 export_runs.py [--out F] [--replay-experiment [DIR]]

The default output is ``runs.csv`` at the root of the experiment.
"""

from __future__ import annotations

import argparse
import csv
import sys
from pathlib import Path

import yaml

HERE = Path(__file__).resolve().parent  # the experiment directory; everything generated lands here

from experiment_base import base

BASE = base(HERE)  # HERE, or the experiment named by --replay-experiment
REFINEMENTS = BASE / "benchmarks-refinements"

CASES = [("full", "paco"), ("accept", "paco-accept"),
         ("reject", "paco-reject"), ("none", "paco-none"),
         ("storm", "storm")]
SIZED = ["accept", "reject", "none"]


def rounds_of(path: Path) -> dict[int, dict]:
    y = yaml.safe_load(path.read_text())
    return {r["round"]: r for r in y.get("rounds", [])}


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--out", default=None)
    a = p.parse_args()
    out_path = Path(a.out) if a.out else BASE / "runs.csv"

    header = (["instance", "nested", "independent", "process_number",
               "dimensions", "mode", "diagonal", "round", "component",
               "response"]
              + [f"seconds_{c}" for c, _ in CASES]
              + [f"{f}_{c}" for c in SIZED
                 for f in ("histories", "open", "decisions")])

    leaders = sorted(REFINEMENTS.rglob("*-paco.yaml"))
    if not leaders:
        print(f"no recorded rounds under {REFINEMENTS}", file=sys.stderr)
        return 2
    rows = disagreements = 0
    with open(out_path, "w", newline="") as fh:
        w = csv.writer(fh)
        w.writerow(header)
        for n, lead in enumerate(leaders, 1):
            key = lead.name[: -len("-paco.yaml")]
            nested = int(lead.parts[-4].split("-")[0])
            independent = int(lead.parts[-3].split("-")[0])
            process_number = int(lead.parts[-2].split("-")[0])
            dimensions, mode = key.split("-", 1)
            played = {}
            for case, suffix in CASES:
                f = lead.with_name(f"{key}-{suffix}.yaml")
                if f.exists():
                    played[case] = rounds_of(f)
            for i, r in sorted(played["full"].items()):
                if "failed" in r:
                    continue
                answers = {r["result"]}
                seconds = {"full": r.get("seconds")}
                sizes = {}
                for case in list(played):
                    if case == "full":
                        continue
                    rr = played[case].get(i)
                    if rr is None or "failed" in rr:
                        continue
                    if rr.get("replayed_result") is None:
                        continue
                    answers.add(rr["replayed_result"])
                    seconds[case] = rr.get("seconds")
                    if case in SIZED and rr.get("histories") is not None:
                        sizes[case] = (rr["histories"], rr["open"], rr["decisions"])
                if len(answers) != 1:
                    print(f"DISAGREEMENT at {key} round {i}: {answers}",
                          file=sys.stderr)
                    disagreements += 1
                    continue
                row = [f"{nested}-{independent}-{process_number}-{key}",
                       nested, independent, process_number, dimensions, mode,
                       nested + independent, i, r["component"],
                       "yes" if answers == {True} else "no"]
                row += [seconds.get(c, "") for c, _ in CASES]
                for c in SIZED:
                    row += list(sizes.get(c, ("", "", "")))
                w.writerow(row)
                rows += 1
            if n % 2000 == 0:
                print(f"[{n}/{len(leaders)}] {rows} rows", flush=True)
    if disagreements:
        print(f"{disagreements} disagreements; the export is not to be "
              "trusted until they are explained", file=sys.stderr)
        return 1
    print(f"{out_path}: {rows} rows over {len(leaders)} instances, "
          "every answered question with one answer")
    return 0


if __name__ == "__main__":
    sys.exit(main())
