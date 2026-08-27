#!/usr/bin/env python
"""Put one checker through the games another has already played.

One checker leads: it draws the components, asks its own questions and moves the
box by its own answers, which is the refinement game of `refinement_game.py`. The
other follows, later and on a quiet machine, asking the recorded questions in the
recorded order and moving the box by the recorded answers, so that both walk one
path and a disagreement is written down rather than sending them apart.

Our search leads, because it answers every instance of the grid in milliseconds,
including the 14 630 whose state space no model checker here can build. The model
checker follows. That way the path always completes and its coverage is a count
over ten rounds rather than the point at which an instance stopped, which is what
happened while it led: a round it could not answer left no box to move and took
the rest of the instance with it.

The pass takes its work from the files the leader has written, so it stops as
soon as it has replayed everything recorded up to the moment it started. Run it
again later and it finds what has appeared since, and only that.

Usage
-----
    python replay.py                              # storm follows paco
    python replay.py --leader storm --follower paco
    python replay.py --timeout 300                # revisit what ran out of time
    python replay.py --dry-run
"""

from __future__ import annotations

import argparse
import re
import sys
import time
from pathlib import Path

import yaml

import refinement_game as game

REFINEMENTS = game.REFINEMENTS

ROUND = re.compile(r"^- round:", re.M)


def count_rounds(path: Path) -> int:
    return len(ROUND.findall(path.read_text()))


def ends_failed(path: Path) -> bool:
    """Whether the last round of the file is one that was never answered."""
    text = path.read_text()
    cut = text.rfind("\n- round:")
    return cut >= 0 and "failed:" in text[cut:]


def outstanding(leader: str, follower: str, rounds_wanted: int):
    """The instances whose replay is missing or shallower than the record."""
    work = []
    for lead in REFINEMENTS.rglob(f"*-{leader}.yaml"):
        stem = lead.name[: -len(f"-{leader}.yaml")]
        fol = lead.with_name(f"{stem}-{follower}.yaml")
        # counted rather than parsed: a full yaml load of every recorded game
        # costs minutes over a grid of this size, and the two questions here are
        # how many rounds a file holds and whether its last one failed
        want = min(count_rounds(lead), rounds_wanted)
        if not want:
            continue
        if fol.exists():
            have_n, have_failed = count_rounds(fol), ends_failed(fol)
            if have_n >= want:
                continue
            if have_failed:
                # it stopped of its own accord, having run out of time once too
                # often. Whether it is worth another try is decided inside the
                # game, by comparing the timeout it hit with the one asked for
                work.append((lead.parent.relative_to(REFINEMENTS) / stem, want))
                continue
            # the recorded run has grown since the replay, so the replay starts
            # again: its later rounds depend on answers it has not seen
            fol.unlink()
        work.append((lead.parent.relative_to(REFINEMENTS) / stem, want))

    def order(item):
        rel = item[0]
        n = int(rel.parts[0].split("-")[0])
        i = int(rel.parts[1].split("-")[0])
        p = int(rel.parts[2].split("-")[0])
        return (n + i, n, i, p, rel.name)

    return sorted(work, key=order)


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--leader", default="paco", help="whose games are replayed")
    p.add_argument("--follower", default="storm", help="who replays them")
    p.add_argument("--rounds", type=int, default=10)
    p.add_argument("--workers", type=int, default=1,
                   help="workers of the search, when it is the follower")
    p.add_argument("--timeout", type=int, default=120,
                   help="seconds a single round may take. A round that runs out "
                        "costs the follower that round alone, the path being "
                        "already written down, and a later pass with a larger "
                        "timeout revisits it")
    p.add_argument("--give-up-after", type=int, default=2,
                   help="consecutive rounds lost to the timeout before the "
                        "instance is left, every further round costing the "
                        "timeout in full")
    p.add_argument("--limit", type=int, default=None)
    p.add_argument("--max-diagonal", type=int, default=None,
                   help="play only the instances whose nested and independent "
                        "splits add to at most this. The sweep moves outwards "
                        "along the diagonals, so a bound here is a scope and "
                        "not a truncation: what it covers, it covers whole")
    p.add_argument("--jobs", type=int, default=1,
                   help="instances at a time. One keeps the timings clean")
    p.add_argument("--dry-run", action="store_true")
    a = p.parse_args()

    work = outstanding(a.leader, a.follower, a.rounds)
    if a.max_diagonal:
        work = [w for w in work
                if int(w[0].parts[0].split("-")[0])
                + int(w[0].parts[1].split("-")[0]) <= a.max_diagonal]
    if a.limit:
        work = work[: a.limit]
    print(f"{len(work)} instances for {a.follower} to replay after {a.leader}",
          flush=True)
    if a.dry_run or not work:
        for rel, want in work[:20]:
            print(f"   {game.rel_to_key(rel)}  {want} rounds")
        return 0

    started = time.time()
    done: dict[str, int] = {}

    def run(item):
        rel, want = item
        return game.play(rel, want, a.follower, a.leader, a.timeout,
                         20260815, a.workers, a.give_up_after)

    if a.jobs > 1:
        from concurrent.futures import ThreadPoolExecutor
        with ThreadPoolExecutor(max_workers=a.jobs) as pool:
            outcomes = list(pool.map(run, work))
    else:
        outcomes = (run(w) for w in work)

    for n, (item, outcome) in enumerate(zip(work, outcomes), 1):
        done[outcome] = done.get(outcome, 0) + 1
        if outcome != "ok" or n % 200 == 0 or n == len(work):
            print(f"[{n}/{len(work)}] {game.rel_to_key(item[0])}: {outcome}   "
                  f"{done}  {round(time.time() - started)} s", flush=True)

    agree = total = unanswered = 0
    for rel, _ in work:
        f = (REFINEMENTS / rel).with_name(f"{rel.name}-{a.follower}").with_suffix(".yaml")
        if not f.exists():
            continue
        for r in yaml.safe_load(f.read_text()).get("rounds", []):
            if "failed" in r:
                unanswered += 1
                continue
            if r.get("replayed_result") is None:
                continue
            total += 1
            agree += r["result"] == r["replayed_result"]
    print(f"\n{agree} of {total} replayed rounds agree with the recorded run, "
          f"{unanswered} rounds unanswered by {a.follower}")
    if agree != total:
        print("DISAGREEMENTS: look for rounds where result differs from "
              "replayed_result. Every one of them is a bug in one of the two "
              "and none of them is noise")
    return 0


if __name__ == "__main__":
    sys.exit(main())
