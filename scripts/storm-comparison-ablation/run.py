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

With `--rebuild` the same command relaunches the pipeline from nothing
instead: it fetches the pinned benchmark, generates a fresh grid with the
stable seed, computes the optima, asks Storm for the reference bounds
when it is on the PATH, plays the games with the search, replays them
with every follower, and aggregates the numbers and the figures. The
target folder is `rebuilt_experiment/` beside the scripts, or
`--experiment DIR`; every stage resumes, so an interrupted rebuild is
run again and continues. `--max-diagonal` and `--dimensions` scope the
grid, and the full grid without them is the scale of the paper, days
included.

    run.py --rebuild --max-diagonal 3 --dimensions 2 3     a taste
    run.py --rebuild                                       the paper scale

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


def rebuild(a) -> int:
    import setup as setup_stage
    if setup_stage.fetch_benchmark():
        return 2
    ws = Path(a.experiment).resolve() if a.experiment else HERE / "rebuilt_experiment"
    ws.mkdir(parents=True, exist_ok=True)
    dims = [str(d) for d in (a.dimensions or range(2, 11))]

    def stage(script, *args):
        cmd = [sys.executable, str(HERE / script), "--replay-experiment", str(ws)]
        cmd += [str(x) for x in args]
        subprocess.run(cmd, check=True)

    try:
        if a.full or not a.max_diagonal:
            stage("make_bpmn_cpi.py", "--all", "--dimensions", *dims,
                  "--mode", "all", "--seed", a.seed)
        else:
            files = sorted((HERE / "process-impact-benchmarks"
                            / "generated_processes").glob(
                                "generated_processes_full_*_*.txt"))
            for f in files:
                n, i = map(int, f.stem.split("_")[-2:])
                if n + i > a.max_diagonal:
                    continue
                shapes = len([l for l in f.read_text().splitlines() if l.strip()])
                stage("make_bpmn_cpi.py", "--nested", n, "--independent", i,
                      "--process-number", *range(1, shapes + 1),
                      "--dimensions", *dims, "--mode", "all", "--seed", a.seed)
        stage("exact_optima.py", "--all")
        have_storm = shutil.which("storm") is not None
        if have_storm and not a.skip_bounds:
            stage("compute_bounds.py", "--all", "--timeout", a.timeout)
        elif not have_storm:
            print("storm is not on the PATH: the reference bounds and the "
                  "storm replay are skipped", flush=True)
        stage("refinement_game.py", "--all", "--checker", "paco",
              "--rounds", a.rounds, "--timeout", a.timeout)
        for follower in (FOLLOWERS if have_storm else FOLLOWERS[:-1]):
            stage("replay.py", "--follower", follower,
                  "--rounds", a.rounds, "--timeout", a.timeout)
        stage("make_results.py")
        try:
            stage("make_figures.py")
        except subprocess.CalledProcessError:
            print("figures skipped; plotly and kaleido are needed for them",
                  flush=True)
    except subprocess.CalledProcessError as e:
        print(f"a stage failed and the rebuild stops there; running the same "
              f"command again resumes: {e}", file=sys.stderr)
        return 1
    print(f"\nrebuilt experiment at {ws}")
    return 0


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
    p.add_argument("--rebuild", action="store_true",
                   help="relaunch the pipeline from nothing into the target "
                        "folder instead of replaying the recorded campaign")
    p.add_argument("--dimensions", type=int, nargs="+", default=None,
                   help="rebuild only: the impact dimensions of the grid "
                        "(default 2 through 10)")
    p.add_argument("--seed", type=int, default=20260815)
    p.add_argument("--skip-bounds", action="store_true",
                   help="rebuild only: leave the Storm reference bounds out")
    a = p.parse_args()

    if not BINARY.exists():
        print(f"{BINARY} does not exist; build it first:\n    cargo build --release",
              file=sys.stderr)
        return 2
    if a.rebuild:
        return rebuild(a)

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
