#!/usr/bin/env python
"""The refinement game: how the thresholds of the comparison are chosen.

A threshold picked at random inside the box of the per-component extrema is
almost always trivial, answered without looking at the process. The refinement
game instead walks towards the frontier, one component at a time, so that every
question it asks is one the checker has to work for, and so that the same
questions can be put to every competitor.

Fix a process. Storm gives, per component, the range of the expected impact over
the policies,

    low_j = min over the policies,      up_j = max over the policies,

and every policy sits below up componentwise, so a threshold at up is a question
with a trivial yes. Starting from that box, each round bisects one component:

    Delta_i = (up_i - low_i) / (up - low)          componentwise, all one at the start
    P_i     = Delta_i / sum(Delta_i)               a distribution over the components
    J       ~ P_i                                  a component, wider ranges likelier
    B_i     = up_i everywhere except at J, where it is (up_i[J] + low_i[J]) / 2
    result  = check(process, B_i)                  does a policy meet B_i
    if result:  up_{i+1}[J]  = the midpoint        the yes region is smaller than we thought
    else:       low_{i+1}[J] = the midpoint        the no region is larger

The component is drawn rather than cycled, and drawn in proportion to how much of
its range is left, so the components that have been refined least are the ones
most likely to be refined next. After a few rounds the constraints on the other
components stop being vacuous and the question becomes genuinely
multi-objective.

Every round is recorded: which component was drawn, the threshold, the answer,
and the time the checker took. That is what lets a competitor be asked exactly
the same questions later, and it is what makes the run resumable: ten rounds
today, more tomorrow, starting from the box where it stopped.

The checker for now is Storm on the trail encoding, restricted to deterministic
policies:

    storm --prism <trail model> --prop 'multi(...)'
          --multiobjective:purescheds positional --exact

The trail encoding is what makes the restriction mean our class of policies, and
--exact is not optional: in floating point the query can answer no on a threshold
a policy attains exactly. Thresholds are carried as exact rationals for the same
reason.

The results mirror the layout of the instances, with the checker in the name:

    bpmn-cpi-benchmarks/3-nested/2-independent/1-process_number/4-random.yaml
    benchmarks-refinements/3-nested/2-independent/1-process_number/4-random-storm.yaml

Usage
-----
    python refinement_game.py --all --rounds 10
    python refinement_game.py 3-2-1-4-random --rounds 10
    python refinement_game.py 3-2-1-4-random --rounds 20        # resumes, adds ten
    python refinement_game.py 3-2-1-4-random --checker paco --replay storm

The last form is how a competitor is measured: it asks the very questions the
recorded run of the other checker asked, round by round, and records its own
answers and its own times beside them.
"""

from __future__ import annotations

import argparse
import os
import random
import re
import subprocess
import sys
import time
from fractions import Fraction
from pathlib import Path

import yaml

HERE = Path(__file__).resolve().parent.parent  # the repository root
INSTANCES = HERE / "bpmn-cpi-benchmarks"
TRAIL = HERE / "prism-trail-encodings"
BOUNDS = HERE / "benchmarks-optima"
REFINEMENTS = HERE / "benchmarks-refinements"

RESULT = re.compile(r"Result \(for initial states\):\s*(true|false)")
CHECK_TIME = re.compile(r"Time for model checking:\s*([\d.]+)s")
STATES = re.compile(r"States:\s+(\d+)")

STORM_FLAGS = ["--multiobjective:purescheds", "positional", "--exact"]

PACO = HERE / "target" / "release" / "sdcpi"
PACO_ANSWER = re.compile(r"^answer\s+(yes|no)\s*$", re.M)
PACO_SECONDS = re.compile(r"^seconds\s+([\d.eE+-]+)\s*$", re.M)
PACO_SIZE = {
    "histories": re.compile(r"^histories won\s+(\d+)", re.M),
    "open": re.compile(r"^open won\s+(\d+)", re.M),
    "decisions": re.compile(r"^decisions won\s+(\d+)", re.M),
}
# the ablations, named as checkers so that each writes its own file beside the
# others. The plain name is the algorithm entire, upper bound and lower bound
# both; every other name says which of the two is left out. Nothing here removes
# the failure criterion that a frontier which has already paid more than the
# threshold cannot be completed, since that is arithmetic and not a bound
ABLATION = {
    "paco": None,
    "paco-accept": "accept",   # the upper bound alone
    "paco-reject": "reject",   # the lower bound alone
    "paco-none": "none",       # neither, the accumulated cost still deciding
}
# the search compares two floating point sums, and a threshold sitting exactly
# on one of them falls either way. Only a component whose range is a single
# point puts a threshold there, and its constraint is met by every policy in any
# case, so the slack below decides nothing that is not already decided
PACO_EPSILON = "1e-12"


def key_to_rel(key: str) -> Path:
    nested, independent, number, rest = key.split("-", 3)
    return (Path(f"{nested}-nested") / f"{independent}-independent"
            / f"{number}-process_number" / rest)


def rel_to_key(rel: Path) -> str:
    p = rel.parts
    return f"{p[0].split('-')[0]}-{p[1].split('-')[0]}-{p[2].split('-')[0]}-{rel.stem}"


def ensure_trail_model(rel: Path) -> Path:
    """Translate with the decision trail, unless the model is already current."""
    src = (INSTANCES / rel).with_suffix(".yaml")
    model = (TRAIL / rel).with_suffix(".prism")
    if not model.exists() or model.stat().st_mtime < src.stat().st_mtime:
        subprocess.run([sys.executable, str(HERE / "to_prism.py"), str(src),
                        "--history"], check=True, capture_output=True)
    return model


def frac(x) -> Fraction:
    """Exact, and exactly what was written: no float is ever widened here."""
    return Fraction(str(x))


def decimal(v: Fraction) -> str:
    """A rational as the decimal it is, when it is one.

    Every threshold here is a bound storm printed, halved a few times, so its
    denominator is a power of two times a power of ten and the decimal is
    finite. Anything else is written with enough digits to be indistinguishable.
    """
    num, den = v.numerator, v.denominator
    d = den
    twos = fives = 0
    while d % 2 == 0:
        d //= 2
        twos += 1
    while d % 5 == 0:
        d //= 5
        fives += 1
    if d != 1:
        return f"{float(v):.17g}"
    digits = max(twos, fives)
    scaled = num * 10 ** digits // den
    s = str(abs(scaled)).rjust(digits + 1, "0")
    body = s if digits == 0 else f"{s[:-digits]}.{s[-digits:]}"
    return ("-" if scaled < 0 else "") + body


def storm_check(model: Path, root: int, bound: list[Fraction],
                live: list[int], timeout: int):
    """One achievability query, from the model file, every time.

    The model is built again at every round, and the building is inside the time
    the round records. That is deliberate and it is the whole point of the
    comparison: our search never builds the process, it explores it on the fly,
    so a competitor that built it once and answered ten questions against the
    result would be spared exactly the cost the paper is about. Each round is a
    separate question put to a tool that starts from the model file, which is
    also how anyone would use it.

    Returns (answer, seconds, checking seconds, states).
    """
    # only the components that have something to say. A component whose range
    # is a single point holds that value under every policy, so its constraint
    # is satisfied by all of them and asking for it changes no answer. Asking
    # for it anyway is not merely wasteful: a component whose impact is
    # identically zero gives the objective a bound of zero on a reward that is
    # always zero, and storm 1.13.0 segfaults on that query under purescheds
    prop = "multi(" + ", ".join(
        f'R{{"impact{j}"}}<={decimal(bound[j])} [ F n{root}=-2 ]'
        for j in live) + ")"
    started = time.time()
    try:
        out = subprocess.run(["storm", "--prism", str(model), "--prop", prop]
                             + STORM_FLAGS, capture_output=True, text=True,
                             timeout=timeout)
    except subprocess.TimeoutExpired:
        return None, float(timeout), None, None, None
    seconds = time.time() - started
    answer = RESULT.search(out.stdout)
    if answer is None:
        first = next((l for l in out.stdout.splitlines() + out.stderr.splitlines()
                      if "ERROR" in l), "storm returned no result")
        raise RuntimeError(first.strip()[:200])
    inner = CHECK_TIME.search(out.stdout)
    states = STATES.search(out.stdout)
    return (answer.group(1) == "true", seconds,
            float(inner.group(1)) if inner else None,
            int(states.group(1)) if states else None, None)


def paco_check(key: str, bound: list[Fraction], workers: int, timeout: int,
               ablation: str | None = None):
    """The same question, put to our own search, and timed the same way.

    Launch to return of a process that starts from the instance file, exactly as
    the model checker is timed, so that what is compared is two answers to one
    question and not two ways of accounting for the work.
    """
    cmd = [str(PACO), "search", key,
           "--B", ",".join(decimal(b) for b in bound),
           "--root", str(INSTANCES), "--workers", str(workers),
           "--epsilon", PACO_EPSILON, "--print-size", "1"]
    if ablation:
        cmd += ["--ablation", ablation]
    started = time.time()
    try:
        out = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return None, float(timeout), None, None, None
    seconds = time.time() - started
    answer = PACO_ANSWER.search(out.stdout)
    if answer is None:
        first = next((l for l in (out.stdout + out.stderr).splitlines()
                      if l.strip()), "the search returned no answer")
        raise RuntimeError(first.strip()[:200])
    inner = PACO_SECONDS.search(out.stdout)
    size = {k: int(m.group(1)) for k, r in PACO_SIZE.items()
            if (m := r.search(out.stdout))}
    return (answer.group(1) == "yes", seconds,
            float(inner.group(1)) if inner else None, None, size or None)


def as_text(v) -> str:
    """A rational, written so that reading it back loses nothing."""
    return f"{v.numerator}/{v.denominator}"


def load_state(out: Path):
    """The rounds played so far, or nothing."""
    if not out.exists():
        return None
    return yaml.safe_load(out.read_text())


def play(rel: Path, rounds: int, checker: str, replay: str | None,
         timeout: int, seed: int, workers: int = 1,
         give_up_after: int = 2) -> str:
    key = rel_to_key(rel)
    src = (INSTANCES / rel).with_suffix(".yaml")
    bnd = (BOUNDS / rel).with_suffix(".yaml")
    if not bnd.exists():
        return "no bounds"
    b = yaml.safe_load(bnd.read_text())
    if "bounds" not in b:
        return "bounds failed"
    inst = yaml.safe_load(src.read_text())
    root = inst["tree"][0]["id"]

    low0 = [frac(c["min"]) for c in b["bounds"]]
    up0 = [frac(c["max"]) for c in b["bounds"]]
    # the maximum before it was opened by the padding. It is what says whether a
    # component has a range at all, the padded one being positive even where the
    # component holds the same value under every policy
    raw = [frac(c.get("max_raw", c["max"])) for c in b["bounds"]]
    flat = [raw[j] <= low0[j] for j in range(len(low0))]
    width0 = [u - l for u, l in zip(up0, low0)]
    if all(flat):
        # every policy has the same expected impact in every component, which
        # happens when the process has no choice at all, so the game has no
        # question to ask. A component that is flat on its own is not a reason
        # to skip the instance: it is simply never drawn, below
        return "degenerate"

    out = (REFINEMENTS / rel).with_name(f"{rel.name}-{checker}").with_suffix(".yaml")
    state = load_state(out)
    played = state["rounds"] if state else []
    retry = None
    if played and "failed" in played[-1]:
        last = played[-1]
        timed_out = last.get("timeout_seconds")
        if timed_out is None or timeout > timed_out:
            # the same question, asked again: with more time if it ran out of
            # time, and plainly if it failed for another reason, since that
            # reason may have been fixed since
            retry = (last["component"], [frac(x) for x in last["threshold"]])
            played = played[:-1]
        else:
            return "skip"
    if len(played) >= rounds:
        return "skip"

    # resume from the box the recorded rounds left behind
    low = [frac(x) for x in state["low_now"]] if state else list(low0)
    up = [frac(x) for x in state["up_now"]] if state else list(up0)

    script = None
    if replay:
        other = (REFINEMENTS / rel).with_name(f"{rel.name}-{replay}").with_suffix(".yaml")
        if not other.exists():
            return f"no {replay} run to replay"
        script = yaml.safe_load(other.read_text())["rounds"]
        if len(script) < rounds:
            rounds = len(script)

    model = None if checker in ABLATION else ensure_trail_model(rel)
    k = len(low)
    # every component is put to the model checker, the padded edge binding on
    # nobody, except one whose impact is identically zero: there the bound is
    # zero on a reward that is always zero, and storm 1.13.0 takes signal 11 on
    # that query. Our search is given the whole vector in every case
    live = [j for j in range(k) if raw[j] > 0]
    consecutive = 0
    states = state.get("states") if state else None
    for i in range(len(played), rounds):
        if retry is not None and i == len(played):
            j, bound = retry
            retry = None
        elif script is not None:
            # the competitor is asked the recorded question, and the box moves
            # by the recorded answer, so that both runs walk the same path
            r = script[i]
            j = r["component"]
            bound = [frac(x) for x in r["threshold"]]
        else:
            # a component that holds one value under every policy has nothing
            # to bisect, and its share of the distribution is zero, so it is
            # never drawn
            delta = [Fraction(0) if flat[j] else (up[j] - low[j]) / width0[j]
                     for j in range(k)]
            total = sum(delta)
            p = [d / total for d in delta]
            draw = Fraction(random.Random(f"{seed}:{key}:{i}").random())
            acc, j = Fraction(0), k - 1
            for c in range(k):
                acc += p[c]
                if draw < acc:
                    j = c
                    break
            bound = list(up)
            bound[j] = (up[j] + low[j]) / 2

        try:
            if checker in ABLATION:
                answer, seconds, inner, n, size = paco_check(
                    key, bound, workers, timeout, ABLATION[checker])
            else:
                answer, seconds, inner, n, size = storm_check(
                    model, root, bound, live, timeout)
            states = states or n
        except RuntimeError as e:
            record = {"round": i, "component": j,
                      "threshold": [as_text(x) for x in bound],
                      "failed": str(e)}
            played.append(record)
            break
        if answer is None:
            record = {"round": i, "component": j,
                      "threshold": [as_text(x) for x in bound],
                      "failed": f"timeout after {timeout} s",
                      "timeout_seconds": timeout,
                      "seconds": seconds}
            if script is None:
                # the game is led from here, so an unanswered round leaves no
                # box to move and the instance stops. A later run with a larger
                # timeout picks this very question up again and carries on
                played.append(record)
                break
            # a follower is walking a path that is already written down, so a
            # round it cannot answer costs it that round and nothing more: the
            # next question does not depend on it. Coverage becomes a count over
            # ten rounds rather than the point where the instance stopped
            record["replayed_result"] = script[i].get("result")
            played.append(record)
            consecutive += 1
            if consecutive >= give_up_after:
                # an instance it has failed this often it will keep failing, and
                # every further round costs the timeout in full
                break
            continue
        consecutive = 0

        ref = script[i].get("result") if script else None
        record = {
            "round": i,
            "component": j,
            "threshold": [as_text(x) for x in bound],
            "result": bool(answer),
            "seconds": round(seconds, 4),
            "check_seconds": inner,
            "states": states,
        }
        if size:
            # how large the partial strategy is, on a positive answer: the
            # histories of the winning frontier, how many are left open to the
            # bound rather than to a decision, and how many decisions it takes
            record.update(size)
        if script:
            record["replayed_result"] = ref
            if "failed" in script[i]:
                # the recorded run never answered this one. We ask it anyway,
                # which is the interesting case: the question the model checker
                # could not settle, settled here, and the record says so
                record["replayed_failed"] = script[i]["failed"]
        played.append(record)
        # the answer moves one side of the interval of the drawn component to
        # the midpoint, and leaves every other component where it was
        decide = answer if script is None else ref
        if decide is None:
            # nothing follows an unanswered round in the recorded run, so there
            # is no box to move and no further question to replay
            break
        moved = bound[j]
        if decide:
            up[j] = moved
        else:
            low[j] = moved

    write(out, key, inst, b, checker, replay, low0, up0, low, up, played, seed,
          states, timeout, live, workers)
    return "ok" if played and "failed" not in played[-1] else "fail"


def write(out: Path, key, inst, b, checker, replay, low0, up0, low, up, played,
          seed, states, timeout, live, workers):
    out.parent.mkdir(parents=True, exist_ok=True)
    doc = {
        "instance": key,
        "dimensions": inst["dimensions"],
        "mode": inst["mode"],
        "tasks": inst["tasks"],
        "nodes": inst["nodes"],
        "checker": checker,
        "box": "benchmarks-optima, upper bounds padded",
        "config": (f"sdcpi --workers {workers} --epsilon {PACO_EPSILON}"
                   + (f" --ablation {ABLATION[checker]}" if ABLATION.get(checker) else "")
                   if checker in ABLATION else
                   "storm on the trail encoding, "
                   "--multiobjective:purescheds positional --exact"),
        "encoding": "instance" if checker in ABLATION else "trail",
        "seed": seed,
        "states": states,
        "round_timeout": timeout,
        "replay_of": replay,
        "low_0": [as_text(x) for x in low0],
        "up_0": [as_text(x) for x in up0],
        "low_now": [as_text(x) for x in low],
        "up_now": [as_text(x) for x in up],
        "components_asked": live,
        "rounds_played": len(played),
        "rounds": played,
    }
    out.write_text(yaml.safe_dump(doc, sort_keys=False, default_flow_style=False))


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("keys", nargs="*", help="instance keys, as in 3-2-1-4-random")
    p.add_argument("--all", action="store_true", help="every instance with bounds")
    p.add_argument("--rounds", type=int, default=10,
                   help="how many rounds in total, counting those already "
                        "recorded, so a larger number resumes and continues")
    p.add_argument("--checker", default="storm",
                   help="storm, or paco for our own search. The name also "
                        "names the file the run is written to")
    p.add_argument("--workers", type=int, default=1,
                   help="workers of the search, when the checker is paco. One "
                        "is the like for like comparison with a model checker "
                        "that answers a query on one core")
    p.add_argument("--replay", default=None,
                   help="ask the questions a recorded run of this other checker "
                        "asked, instead of drawing them")
    p.add_argument("--timeout", type=int, default=120,
                   help="seconds a single round may take. The slowest round "
                        "storm actually completed on the grid took 110 s, so "
                        "this keeps what it can do and cuts what it cannot. A "
                        "round that runs out is recorded with the timeout it "
                        "hit and stops the instance there; running again with a "
                        "larger timeout takes that same question up again")
    p.add_argument("--seed", type=int, default=20260815,
                   help="the draws of the component are derived from this and "
                        "from the key of the instance, so a run repeats")
    p.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) - 4))
    a = p.parse_args()

    if a.all:
        # by the sum of the two structural counts: every instance whose nested
        # and independent splits add to two, then every instance that adds to
        # three, and so on. The sweep therefore moves outwards along the
        # diagonals of the grid, so a campaign stopped early has covered whole
        # diagonals rather than a corner, and the cheap ones come first
        def order(rel: Path):
            n = int(rel.parts[0].split("-")[0])
            i = int(rel.parts[1].split("-")[0])
            p_ = int(rel.parts[2].split("-")[0])
            return (n + i, n, i, p_, rel.name)
        rels = sorted((q.relative_to(BOUNDS).with_suffix("")
                       for q in BOUNDS.rglob("*.yaml")), key=order)
    else:
        rels = [key_to_rel(k) for k in a.keys]
    if not rels:
        p.error("give some keys or --all")

    from concurrent.futures import ThreadPoolExecutor
    done: dict[str, int] = {}
    started = time.time()
    with ThreadPoolExecutor(max_workers=a.jobs) as pool:
        for n, (rel, outcome) in enumerate(zip(rels, pool.map(
                lambda r: play(r, a.rounds, a.checker, a.replay, a.timeout,
                               a.seed, a.workers),
                rels)), 1):
            done[outcome] = done.get(outcome, 0) + 1
            if outcome != "skip" or n % 200 == 0:
                print(f"[{n}/{len(rels)}] {rel_to_key(rel)}: {outcome}   "
                      f"{done}  {round(time.time() - started)} s", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
