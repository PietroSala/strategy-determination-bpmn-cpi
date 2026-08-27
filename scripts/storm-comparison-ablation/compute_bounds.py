#!/usr/bin/env python
"""Compute, with Storm, the range of the expected impact of every instance.

For each component of the impact vector the range is the pair

    min over the strategies of the expected impact of that component,
    max over the strategies of the same,

which is what a threshold has to sit inside for the question to be worth asking:
a bound above every maximum is a yes for any strategy, one below some minimum is
a no for all of them. The two numbers are therefore the scale on which the
thresholds of the experiments are placed, and they are computed once, here,
outside the time the experiments measure.

Storm is asked for all of them in ONE call per instance, the properties being
passed together, so the model is built once instead of once per query. Model
construction dominates, so this is most of the cost.

The results mirror the layout of the instances:

    bpmn-cpi-benchmarks/3-nested/2-independent/1-process_number/4-random.yaml
    benchmarks-bounds/3-nested/2-independent/1-process_number/4-random.yaml

Usage
-----
    python compute_bounds.py --all --timeout 300
    python compute_bounds.py 3-2-1-4-random
    python compute_bounds.py --nested 1 2 3 --dimensions 2 3 --mode random

An instance whose bounds are already on disk is skipped, so a run can be stopped
and resumed. An instance that times out or fails is recorded with the reason in
place of the numbers, so that the grid says what happened rather than going
quiet.
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

HERE = Path(__file__).resolve().parent  # the experiment directory; everything generated lands here

from experiment_base import base

BASE = base(HERE)  # HERE, or the experiment named by --replay-experiment
INSTANCES = BASE / "bpmn-cpi-benchmarks"
ENCODINGS = BASE / "prism-benchmark-encodings"
BOUNDS = BASE / "benchmarks-bounds"

RESULT = re.compile(r"Result \(for initial states\):\s*([-\d.eE+]+)")


def key_to_rel(key: str) -> Path:
    nested, independent, number, dimensions, mode = key.split("-", 4)
    return (Path(f"{nested}-nested") / f"{independent}-independent"
            / f"{number}-process_number" / f"{dimensions}-{mode}")


def rel_to_key(rel: Path) -> str:
    parts = rel.parts
    n = parts[0].split("-")[0]
    i = parts[1].split("-")[0]
    p = parts[2].split("-")[0]
    return f"{n}-{i}-{p}-{rel.stem}"


def ensure_model(rel: Path) -> Path:
    """Translate the instance if its model is missing or out of date.

    The date matters, and not only the presence. An instance that is generated
    again, with another seed or another rule, leaves behind the model of the
    instance it replaced, and a run that trusts the file it finds measures a
    process nobody asked about. Regenerating whenever the model is not newer
    than the instance costs one translation and removes the whole class of
    mistake.
    """
    src = (INSTANCES / rel).with_suffix(".yaml")
    model = (ENCODINGS / rel).with_suffix(".prism")
    if not model.exists() or model.stat().st_mtime < src.stat().st_mtime:
        subprocess.run([sys.executable, str(HERE / "to_prism.py"), str(src)],
                       check=True, capture_output=True)
    return model


def storm_bounds(model: Path, root_id: int, dimensions: int, timeout: int):
    """One call, two properties per component, in the order min, max."""
    props = "; ".join(
        f'R{{"impact{i}"}}{d}=? [ F n{root_id}=-2 ]'
        for i in range(dimensions) for d in ("min", "max"))
    started = time.time()
    try:
        out = subprocess.run(["storm", "--prism", str(model), "--prop", props],
                             capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return None, f"timeout after {timeout} s", timeout
    elapsed = time.time() - started
    values = [float(m) for m in RESULT.findall(out.stdout)]
    if len(values) != 2 * dimensions:
        first_error = next((l for l in out.stdout.splitlines() + out.stderr.splitlines()
                            if "ERROR" in l), "storm returned no result")
        return None, first_error.strip()[:200], elapsed
    return [(values[2 * i], values[2 * i + 1]) for i in range(dimensions)], None, elapsed


def write_bounds(rel: Path, key: str, meta: dict, ranges, failure, elapsed) -> Path:
    out = (BOUNDS / rel).with_suffix(".yaml")
    out.parent.mkdir(parents=True, exist_ok=True)
    lines = [
        f"# {key}",
        f"instance: {key}",
        f"dimensions: {meta['dimensions']}",
        f"mode: {meta['mode']}",
        f"tasks: {meta['tasks']}",
        f"nodes: {meta['nodes']}",
        f"tool: storm",
        f"seconds: {round(elapsed, 3)}",
    ]
    if failure is None:
        lines.append("bounds:")
        for i, (lo, hi) in enumerate(ranges):
            lines.append(f"  - component: {i}")
            lines.append(f"    min: {lo}")
            lines.append(f"    max: {hi}")
    else:
        lines.append(f"failed: \"{failure}\"")
    out.write_text("\n".join(lines) + "\n")
    return out


def one(rel: Path, timeout: int, force: bool) -> str:
    out = (BOUNDS / rel).with_suffix(".yaml")
    if out.exists() and not force:
        return "skip"
    src = (INSTANCES / rel).with_suffix(".yaml")
    meta = yaml.safe_load(src.read_text())
    root_id = meta["tree"][0]["id"]
    model = ensure_model(rel)
    ranges, failure, elapsed = storm_bounds(model, root_id, meta["dimensions"], timeout)
    write_bounds(rel, rel_to_key(rel), meta, ranges, failure, elapsed)
    return "ok" if failure is None else "fail"


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("keys", nargs="*", help="instance keys, as in 3-2-1-4-random")
    p.add_argument("--all", action="store_true", help="every instance on disk")
    p.add_argument("--timeout", type=int, default=300, help="seconds per instance")
    p.add_argument("--force", action="store_true", help="recompute what is already there")
    p.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) - 4),
                   help="instances to run at once. Storm builds a model on one "
                        "core, so the run scales with this until memory does not")
    a = p.parse_args()

    if a.all:
        rels = sorted((q.relative_to(INSTANCES).with_suffix("")
                       for q in INSTANCES.rglob("*.yaml")),
                      key=lambda r: (int(r.parts[0].split("-")[0])
                                     * int(r.parts[1].split("-")[0]), str(r)))
    else:
        rels = [key_to_rel(k) for k in a.keys]
    if not rels:
        p.error("give some keys or --all")

    done = {"ok": 0, "fail": 0, "skip": 0}
    started = time.time()
    with ThreadPoolExecutor(max_workers=a.jobs) as pool:
        # the work is a subprocess per instance, so threads are the right kind of
        # worker here: they wait, they do not compute
        for n, (rel, outcome) in enumerate(
                zip(rels, pool.map(lambda r: one(r, a.timeout, a.force), rels)), 1):
            done[outcome] += 1
            if outcome != "skip" or n % 500 == 0:
                print(f"[{n}/{len(rels)}] {rel_to_key(rel)}: {outcome}"
                      f"  (ok {done['ok']}, failed {done['fail']}, skipped {done['skip']},"
                      f" {round(time.time() - started)} s)", flush=True)


if __name__ == "__main__":
    main()
