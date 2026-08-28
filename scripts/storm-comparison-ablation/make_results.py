#!/usr/bin/env python3
"""Aggregate the refinement games into the numbers of the record.

One pass over ``benchmarks-refinements`` produces ``results.json``, and
everything downstream, the figures of the experimental section included, reads
that file rather than the hundred thousand round records. The experiments are
still running while the record is being read, so the pipeline is one command
per update:

    python3 make_results.py && python3 make_figures.py

The five configurations aggregated here, named as the files name them:

    paco          the search, both bounds, the leader that drew the questions
    storm         the model checker, replaying the same questions
    paco-accept   the search with the upper bound alone
    paco-reject   the search with the lower bound alone
    paco-none     the search with neither, the accumulated cost still deciding

Rounds are compared pairwise at the same index of the same instance, which the
replay design guarantees to be the same question. A round with no answer, from
the timeout of whichever configuration, is excluded from every timing sum and
counted apart, so a configuration is never charged the seconds of a question it
did not answer, and is charged the loss instead.

Parsing is by regular expression rather than a YAML loader: the files are written
by one script in one shape, and a full parse of a hundred thousand of them costs
minutes where this costs seconds.
"""

from __future__ import annotations

import json
import re
import statistics
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent  # the experiment directory; everything generated lands here

from experiment_base import base

BASE = base(HERE)  # HERE, or the experiment named by --replay-experiment
REF = BASE / "benchmarks-refinements"
OUT = BASE / "results.json"

BLOCK = re.compile(r"^- round:.*?(?=^- round:|\Z)", re.M | re.S)
RESULT = re.compile(r"^  result: (\w+)", re.M)
SECONDS = re.compile(r"^  seconds: ([\d.eE+-]+)", re.M)
DECISIONS = re.compile(r"^  decisions: (\d+)", re.M)
OPEN = re.compile(r"^  open: (\d+)", re.M)
FAILED = re.compile(r"^  failed:", re.M)

CONFIGS = ["paco", "storm", "paco-accept", "paco-reject", "paco-none"]


def diagonal_of(path: Path) -> int:
    parts = path.parts
    nested = int(parts[-4].split("-")[0])
    independent = int(parts[-3].split("-")[0])
    return nested + independent


def read_rounds(path: Path):
    """The rounds of one file: (result, seconds, decisions, open) per round,
    with result None where the round was never answered."""
    rounds = []
    for block in BLOCK.findall(path.read_text()):
        if FAILED.search(block):
            rounds.append((None, None, None, None))
            continue
        r = RESULT.search(block)
        s = SECONDS.search(block)
        if not r or not s:
            continue
        d = DECISIONS.search(block)
        o = OPEN.search(block)
        rounds.append((r.group(1) == "true", float(s.group(1)),
                       int(d.group(1)) if d else None,
                       int(o.group(1)) if o else None))
    return rounds


def pct(values, q):
    if not values:
        return None
    values = sorted(values)
    return values[min(len(values) - 1, int(q * len(values)))]


def main() -> None:
    started = time.time()
    # every instance, keyed by its path stem, with the rounds of each
    # configuration that has played it
    instances: dict[str, dict] = {}
    for leader in REF.rglob("*-paco.yaml"):
        stem = str(leader)[: -len("-paco.yaml")]
        entry = {"diagonal": diagonal_of(leader), "paco": read_rounds(leader)}
        for cfg in CONFIGS[1:]:
            p = Path(f"{stem}-{cfg}.yaml")
            if p.exists():
                entry[cfg] = read_rounds(p)
        instances[stem] = entry

    diagonals = sorted({e["diagonal"] for e in instances.values()})

    # -- the game itself, from the leader ---------------------------------
    no_by_round = [0] * 10
    answered_by_round = [0] * 10
    no_by_diagonal = {d: 0 for d in diagonals}
    answered_by_diagonal = {d: 0 for d in diagonals}
    all_yes_by_diagonal = {d: 0 for d in diagonals}
    unfinished = {d: 0 for d in diagonals}
    played = {d: 0 for d in diagonals}
    all_yes = 0
    for e in instances.values():
        d = e["diagonal"]
        played[d] += 1
        rs = e["paco"]
        if any(r[0] is None for r in rs) or len(rs) < 10:
            unfinished[d] += 1
        answers = [r[0] for r in rs if r[0] is not None]
        answered_by_diagonal[d] += len(answers)
        no_by_diagonal[d] += sum(1 for a in answers if a is False)
        if answers and all(answers):
            all_yes += 1
            all_yes_by_diagonal[d] += 1
        for i, r in enumerate(rs[:10]):
            if r[0] is not None:
                answered_by_round[i] += 1
                if r[0] is False:
                    no_by_round[i] += 1

    # -- the model checker against the search -----------------------------
    # A diagonal is COMPLETE when the model checker has a record for every
    # instance of it that the search played. Everything reported of
    # the comparison is read on the complete diagonals alone: a partial one
    # is a biased sample of its cell, and its total seconds would be compared
    # against the full total of the diagonal below it. The flag is computed
    # here rather than written down, so that a diagonal enters the figures
    # and the text of its own accord as the pass finishes it.
    storm = {"agree": 0, "disagree": 0, "by_diagonal": {}, "scatter": [],
             "complete_diagonals": [], "complete": {}}
    sd = storm["by_diagonal"]
    for e in instances.values():
        if "storm" not in e:
            continue
        d = e["diagonal"]
        row = sd.setdefault(d, {
            "instances": 0, "instances_lost": 0, "rounds": 0,
            "unanswered_rounds": 0, "storm_seconds": 0.0, "paco_seconds": 0.0,
            "_storm_rounds": [], "_paco_rounds": []})
        row["instances"] += 1
        lost = False
        st_total = pa_total = 0.0
        both = 0
        for ps, ss in zip(e["paco"], e["storm"]):
            if ss[0] is None:
                row["unanswered_rounds"] += 1
                lost = True
                continue
            if ps[0] is None:
                continue
            row["rounds"] += 1
            both += 1
            storm["agree" if ps[0] == ss[0] else "disagree"] += 1
            row["storm_seconds"] += ss[1]
            row["paco_seconds"] += ps[1]
            row["_storm_rounds"].append(ss[1])
            row["_paco_rounds"].append(ps[1])
            st_total += ss[1]
            pa_total += ps[1]
        if lost:
            row["instances_lost"] += 1
        if both:
            storm["scatter"].append([d, round(pa_total, 4), round(st_total, 3)])
    for d, row in sd.items():
        row["storm_round_median"] = pct(row.pop("_storm_rounds"), 0.5)
        row["paco_round_median"] = pct(row.pop("_paco_rounds"), 0.5)
        row["storm_seconds"] = round(row["storm_seconds"], 1)
        row["paco_seconds"] = round(row["paco_seconds"], 1)
        row["instances_played"] = played[d]
        row["complete"] = row["instances"] == played[d]
    done = sorted(d for d, row in sd.items() if row["complete"])
    storm["complete_diagonals"] = done
    storm["scatter"] = [p for p in storm["scatter"] if p[0] in done]

    # the quadrants: every question put to both, by who answered it. The
    # replay puts every question of the leader to the model checker, its own
    # timeout not stopping it, and puts to it the question the leader could
    # not answer, which closes the game; the pairing by index therefore
    # covers every question asked of both
    quad = {"by_diagonal": {}, "overall": {}}
    for e in instances.values():
        if "storm" not in e:
            continue
        d = e["diagonal"]
        row = quad["by_diagonal"].setdefault(d, {
            "both": 0, "search_only": 0, "storm_only": 0, "neither": 0})
        for ps, ss in zip(e["paco"], e["storm"]):
            pa, sa = ps[0] is not None, ss[0] is not None
            key = ("both" if pa and sa else "search_only" if pa
                   else "storm_only" if sa else "neither")
            row[key] += 1
    for key in ("both", "search_only", "storm_only", "neither"):
        quad["overall"][key] = sum(quad["by_diagonal"][d][key] for d in done)
    storm["quadrants"] = quad

    # the totals of the comparison, over the complete diagonals alone, which
    # is what the section reports; the agreement is counted twice, once here
    # and once over everything replayed, the correctness claim losing nothing
    # by covering a partial diagonal as well
    storm["complete"] = {
        "diagonals": done,
        "instances": sum(sd[d]["instances"] for d in done),
        "rounds": sum(sd[d]["rounds"] for d in done),
        "unanswered_rounds": sum(sd[d]["unanswered_rounds"] for d in done),
        "storm_seconds": round(sum(sd[d]["storm_seconds"] for d in done), 1),
        "paco_seconds": round(sum(sd[d]["paco_seconds"] for d in done), 1),
        "agree": sum(quad["by_diagonal"][d]["both"] for d in done),
    }

    # -- the ablation ------------------------------------------------------
    ablation = {}
    for cfg, key in [("both", "paco"), ("upper", "paco-accept"),
                     ("lower", "paco-reject"), ("neither", "paco-none")]:
        table = {}
        for e in instances.values():
            d = e["diagonal"]
            if d > 10 or key not in e:
                continue
            row = table.setdefault(d, {"instances": 0, "instances_lost": 0,
                                       "rounds": 0, "seconds": 0.0})
            row["instances"] += 1
            rs = e[key]
            if any(r[0] is None for r in rs) or len(rs) < 10:
                row["instances_lost"] += 1
            for r in rs:
                if r[0] is not None:
                    row["rounds"] += 1
                    row["seconds"] += r[1]
        for row in table.values():
            row["seconds"] = round(row["seconds"], 1)
        ablation[cfg] = table

    # -- the size of the partial strategy ---------------------------------
    size = {"by_diagonal": {}, "overall": {}}
    pairs_all = []
    opens = []
    for e in instances.values():
        d = e["diagonal"]
        if d > 10 or "paco-accept" not in e or "paco-reject" not in e:
            continue
        row = size["by_diagonal"].setdefault(d, {
            "pairs": 0, "smaller": 0, "equal": 0, "larger": 0,
            "_up": [], "_no": []})
        for a, r in zip(e["paco-accept"], e["paco-reject"]):
            if a[0] is not True or r[0] is not True:
                continue
            if a[2] is None or r[2] is None:
                continue
            row["pairs"] += 1
            row["_up"].append(a[2])
            row["_no"].append(r[2])
            pairs_all.append((a[2], r[2]))
            if a[3] is not None:
                opens.append(a[3])
            if a[2] < r[2]:
                row["smaller"] += 1
            elif a[2] == r[2]:
                row["equal"] += 1
            else:
                row["larger"] += 1
    for row in size["by_diagonal"].values():
        row["mean_decisions_upper"] = round(statistics.mean(row.pop("_up")), 3)
        row["mean_decisions_noupper"] = round(statistics.mean(row.pop("_no")), 3)
    if pairs_all:
        size["overall"] = {
            "pairs": len(pairs_all),
            "mean_decisions_upper": round(statistics.mean(a for a, _ in pairs_all), 3),
            "mean_decisions_noupper": round(statistics.mean(b for _, b in pairs_all), 3),
            "smaller": sum(1 for a, b in pairs_all if a < b),
            "equal": sum(1 for a, b in pairs_all if a == b),
            "larger": sum(1 for a, b in pairs_all if a > b),
            "mean_open_upper": round(statistics.mean(opens), 3) if opens else None,
        }

    out = {
        "generated": time.strftime("%Y-%m-%d %H:%M:%S"),
        "diagonals": diagonals,
        "leader": {
            "played_by_diagonal": played,
            "unfinished_by_diagonal": unfinished,
            "no_by_round": no_by_round,
            "answered_by_round": answered_by_round,
            "no_by_diagonal": no_by_diagonal,
            "answered_by_diagonal": answered_by_diagonal,
            "all_yes_by_diagonal": all_yes_by_diagonal,
            "all_yes_instances": all_yes,
            "instances": len(instances),
        },
        "storm": storm,
        "ablation": ablation,
        "size": size,
    }
    OUT.write_text(json.dumps(out, indent=1, sort_keys=True))
    print(f"{OUT.name}: {len(instances)} instances, "
          f"storm on {sum(r['instances'] for r in sd.values())}, "
          f"{round(time.time() - started, 1)} s")


if __name__ == "__main__":
    main()
