#!/usr/bin/env python
"""One command over the whole confrontation, as a test of the built library.

`sdcpi test storm-comparison-ablation [options]` lands here, and running
this file directly is the same thing. The default run unpacks the shipped
campaign into a fresh scratch folder, so the copy beside the scripts stays
pristine, forgets the recorded answers of every follower on a small slice,
and re-asks the recorded questions: the three ablated configurations of
the search exercise the binary built at the root of the repository, and
Storm, when it is on the PATH, referees the same questions from the
outside. Every replayed round is compared with the answer the campaign
recorded; a disagreement is a bug in one of the two tools and fails the
run.

    run.py                        a few minutes: diagonals to 3, 20 instances
    run.py --full                 the whole recorded campaign, days for Storm
    run.py --follower storm       one follower alone
    run.py --in-place             run inside default_experiment, mutating it
    run.py --experiment DIR       run inside DIR, mutating it
    run.py --keep                 keep the scratch folder and print its path

The exit code is 0 when every replayed round agrees, 1 on any
disagreement, 2 when the run cannot start.
"""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
BINARY = HERE.parent.parent / "target" / "release" / "sdcpi"
ARCHIVE = HERE / "default_experiment_archive"
FOLLOWERS = ["paco-accept", "paco-reject", "paco-none", "storm"]
AGREEMENT = re.compile(
    r"(\d+) of (\d+) replayed rounds agree with the recorded run, "
    r"(\d+) rounds unanswered")


def unpack_into(scratch: Path) -> Path:
    parts = sorted(ARCHIVE.glob("default_experiment.tar.xz.part-*"))
    if not parts:
        raise SystemExit(f"no archive under {ARCHIVE}; the recorded campaign "
                         "is not in this checkout")
    whole = scratch / "default_experiment.tar.xz"
    with open(whole, "wb") as out:
        for part in parts:
            out.write(part.read_bytes())
    print(f"unpacking the recorded campaign into {scratch}", flush=True)
    with tarfile.open(whole, "r:xz") as tar:
        try:
            tar.extractall(scratch, filter="data")
        except TypeError:  # the filter argument arrived in Python 3.12
            tar.extractall(scratch)
    whole.unlink()
    return scratch / "default_experiment"


def replay(ws: Path, follower: str, a) -> tuple[int, int, int]:
    cmd = [sys.executable, str(HERE / "replay.py"),
           "--replay-experiment", str(ws), "--from-scratch",
           "--follower", follower,
           "--rounds", str(a.rounds), "--timeout", str(a.timeout)]
    if not a.full:
        cmd += ["--max-diagonal", str(a.max_diagonal), "--limit", str(a.limit)]
    print(f"\n=== {follower} ===", flush=True)
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            text=True)
    last = None
    for line in proc.stdout:
        print(line, end="", flush=True)
        m = AGREEMENT.search(line)
        if m:
            last = m
    proc.wait()
    if proc.returncode != 0 or last is None:
        raise SystemExit(f"the replay of {follower} did not finish; see above")
    return int(last.group(1)), int(last.group(2)), int(last.group(3))


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--follower", default="all",
                   choices=["all"] + FOLLOWERS,
                   help="who replays the recorded questions; all runs the "
                        "three ablated configurations of the search, and "
                        "storm when it is on the PATH")
    p.add_argument("--max-diagonal", type=int, default=3)
    p.add_argument("--limit", type=int, default=20,
                   help="instances per follower, from the smallest diagonals")
    p.add_argument("--rounds", type=int, default=10)
    p.add_argument("--timeout", type=int, default=120)
    p.add_argument("--full", action="store_true",
                   help="no caps: the whole recorded campaign, days for Storm")
    p.add_argument("--experiment", default=None,
                   help="run inside this folder, mutating it")
    p.add_argument("--in-place", action="store_true",
                   help="run inside default_experiment, mutating it")
    p.add_argument("--keep", action="store_true",
                   help="keep the scratch folder and print its path")
    a = p.parse_args()

    if not BINARY.exists():
        print(f"{BINARY} does not exist; build it first:\n    cargo build --release",
              file=sys.stderr)
        return 2
    followers = FOLLOWERS if a.follower == "all" else [a.follower]
    if "storm" in followers and shutil.which("storm") is None:
        if a.follower == "all":
            followers = [f for f in followers if f != "storm"]
            print("storm is not on the PATH, so the external referee is "
                  "skipped; the ablations still run", flush=True)
        else:
            print("storm is not on the PATH", file=sys.stderr)
            return 2

    scratch = None
    if a.experiment:
        ws = Path(a.experiment).resolve()
        if not ws.exists():
            print(f"{ws} does not exist", file=sys.stderr)
            return 2
    elif a.in_place:
        ws = HERE / "default_experiment"
        if not ws.exists():
            print(f"{ws} does not exist; run setup.py first", file=sys.stderr)
            return 2
    else:
        scratch = Path(tempfile.mkdtemp(prefix="sdcpi-test-"))
        ws = unpack_into(scratch)

    try:
        verdicts = {f: replay(ws, f, a) for f in followers}
    finally:
        if scratch and not a.keep:
            shutil.rmtree(scratch, ignore_errors=True)
        elif scratch:
            print(f"\nscratch kept at {scratch}")

    print("\n=== verdict ===")
    failed = False
    for f, (agree, total, unanswered) in verdicts.items():
        state = "agrees" if agree == total else "DISAGREES"
        failed = failed or agree != total
        print(f"{f:14} {agree}/{total} rounds {state}, {unanswered} unanswered")
    print("every replayed round agrees with the recorded campaign"
          if not failed else
          "DISAGREEMENT: a bug in one of the two tools; see the rounds above")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
