#!/usr/bin/env python
"""Turn the generated SESE processes into BPMN+CPI trees, one YAML per process.

The processes come from process-impact-benchmarks, which generates them for the
compliance paper: a fully parenthesised binary expression over `^` (XOR),
`||` (parallel) and `,` (sequence), with leaves T1, T2, ... and nothing else on
disk. That benchmark has no nature nodes, no probabilities and no durations,
because the problem it studies has none. Ours has all three, so this script
completes each parse tree:

  impacts     one vector per task, drawn by the six modes of that benchmark,
              reusing its own generate_impacts.generate_vectors so that our
              instances stay comparable with theirs;
  durations   one per task, an integer drawn uniformly in [1, D], where D is
              max(2, ceil(log2 t)) and t is the number of tasks of the process.
              Integers because a PRISM variable is one, and logarithmic because
              the state space a model checker builds is a product over the
              concurrently running tasks of the duration plus two, so durations
              that grow with the process would make the model checkers fail for
              size where we want them to fail, or not fail, for structure. Our
              own search is nearly indifferent to the magnitude, its fast
              forward jumping to the next completion rather than stepping;
  XOR kinds   every XOR node becomes either a choice or a nature node, and the
              two ALTERNATE down the tree. ONE draw fixes the kind of the first
              XOR the visit meets, and from there every XOR takes the opposite
              kind of its nearest XOR ancestor, so two XOR nodes at the same XOR
              depth agree wherever they sit. A nature node carries a probability
              drawn uniformly in (0, 1), the probability of its low branch.

One YAML file per process, named after the key it is indexed by, so that the
grid of the compliance paper survives: nested, independent, process_number,
dimensions, mode.

Usage
-----
    python make_bpmn_cpi.py --nested 3 --independent 2 --dimensions 4 \
                            --mode random --seed 20260815
    python make_bpmn_cpi.py --all --dimensions 2 3 4 5 6 7 8 9 10 \
                            --mode all --seed 20260815

Dimensions start at 2 on purpose: in dimension 1 the threshold question
degenerates into a stochastic shortest path, which is not the problem this
algorithm addresses.
"""

from __future__ import annotations

import argparse
import hashlib
import math
import random
import sys
from dataclasses import dataclass, field
from pathlib import Path

HERE = Path(__file__).resolve().parent  # the experiment directory; everything generated lands here

from experiment_base import base

BASE = base(HERE)  # HERE, or the experiment named by --replay-experiment
BENCH = HERE / "process-impact-benchmarks"
OUT = BASE / "bpmn-cpi-benchmarks"

MODES = [
    "random",
    "bagging_divide",
    "bagging_remove",
    "bagging_remove_divide",
    "bagging_remove_reverse",
    "bagging_remove_reverse_divide",
]

# ---------------------------------------------------------------------------
# the expression, parsed
# ---------------------------------------------------------------------------
# The grammar on disk is small enough to read directly, and reading it here
# keeps this script free of the heavy imports of sese_diagram.py, which pulls in
# pydot and PIL to draw pictures we do not need. The impact modes ARE taken from
# the benchmark, since those have to match it exactly.


@dataclass
class Node:
    kind: str  # task | xor | parallel | sequence
    node_id: int | None = None  # in-order identifier, see assign_ids
    name: str | None = None  # tasks only
    low: "Node | None" = None
    high: "Node | None" = None
    # filled in by the completion pass
    xor_kind: str | None = None  # choice | nature
    probability: float | None = None  # nature only
    impact: list[float] = field(default_factory=list)  # tasks only
    duration: float | None = None  # tasks only


def tokenize(text: str) -> list[str]:
    out, i = [], 0
    while i < len(text):
        c = text[i]
        if c.isspace():
            i += 1
        elif c in "()^,":
            out.append(c)
            i += 1
        elif text.startswith("||", i):
            out.append("||")
            i += 2
        elif c == "T":
            j = i + 1
            while j < len(text) and text[j].isdigit():
                j += 1
            out.append(text[i:j])
            i = j
        else:
            raise ValueError(f"unexpected character {c!r} at {i} in {text!r}")
    return out


def parse(text: str) -> Node:
    """Every internal node is parenthesised, so one pass over the tokens does."""
    tokens = tokenize(text)
    pos = 0

    def expr() -> Node:
        nonlocal pos
        tok = tokens[pos]
        if tok.startswith("T"):
            pos += 1
            return Node(kind="task", name=tok)
        if tok != "(":
            raise ValueError(f"expected ( or a task at token {pos}, found {tok!r}")
        pos += 1
        left = expr()
        op = tokens[pos]
        pos += 1
        right = expr()
        if tokens[pos] != ")":
            raise ValueError(f"expected ) at token {pos}, found {tokens[pos]!r}")
        pos += 1
        kind = {"^": "xor", "||": "parallel", ",": "sequence"}[op]
        return Node(kind=kind, low=left, high=right)

    tree = expr()
    if pos != len(tokens):
        raise ValueError(f"trailing tokens from {pos} in {text!r}")
    return tree


def assign_ids(node: Node, counter: list[int]) -> None:
    """Number the nodes in order, which is the order the semantics uses.

    Every node of the low subtree gets a smaller identifier than the node, and
    every node of the high subtree a larger one, so the identifiers are the
    in-order traversal of the tree. The triggers of the semantics pick the
    witness of least identifier, so carrying them in the file means the file and
    the semantics agree on which node a move acts at.
    """
    if node.kind != "task":
        assign_ids(node.low, counter)
    counter[0] += 1
    node.node_id = counter[0]
    if node.kind != "task":
        assign_ids(node.high, counter)


def nodes_of(node: Node) -> list[Node]:
    if node.kind == "task":
        return [node]
    return nodes_of(node.low) + [node] + nodes_of(node.high)


def tasks_of(node: Node) -> list[Node]:
    if node.kind == "task":
        return [node]
    return tasks_of(node.low) + tasks_of(node.high)


# ---------------------------------------------------------------------------
# the completion: kinds that alternate, probabilities, impacts, durations
# ---------------------------------------------------------------------------


def assign_xor_kinds(node: Node, kind_here: str) -> None:
    """Alternate choice and nature down the tree.

    ONE draw decides the kind of the first XOR the visit meets, and from there
    the kinds alternate: every XOR takes the opposite kind of its nearest XOR
    ancestor, and two XOR nodes at the same XOR depth take the same kind,
    wherever they sit in the tree. Parallel and sequence nodes pass the pending
    kind through untouched, so the alternation is counted in XOR nodes and not
    in tree depth.
    """
    if node.kind == "task":
        return
    if node.kind == "xor":
        node.xor_kind = kind_here
        nxt = "nature" if kind_here == "choice" else "choice"
    else:
        nxt = kind_here
    assign_xor_kinds(node.low, nxt)
    assign_xor_kinds(node.high, nxt)


def draw_probabilities(node: Node, rng: random.Random) -> None:
    """A nature node carries the probability of its low branch."""
    if node.kind == "task":
        return
    if node.xor_kind == "nature":
        node.probability = rng.uniform(0.0, 1.0)
    draw_probabilities(node.low, rng)
    draw_probabilities(node.high, rng)


def attach_impacts_and_durations(tree: Node, dimensions: int, mode: str,
                                 rng: random.Random) -> None:
    sys.path.insert(0, str(BENCH))
    from generate_impacts import generate_vectors  # the six modes of the benchmark

    leaves = tasks_of(tree)
    vectors = generate_vectors(len(leaves), dimensions, mode)
    max_duration = max(2, math.ceil(math.log2(len(leaves)))) if len(leaves) > 1 else 2
    for leaf, vector in zip(leaves, vectors):
        leaf.impact = [round(float(x), 6) for x in vector]
        leaf.duration = rng.randint(1, max_duration)
    return max_duration


# ---------------------------------------------------------------------------
# YAML, written by hand to keep the tree readable and the file dependency-free
# ---------------------------------------------------------------------------


def to_yaml(node: Node, indent: int = 0) -> list[str]:
    pad = "  " * indent
    if node.kind == "task":
        return [
            f"{pad}- task: {node.name}",
            f"{pad}  id: {node.node_id}",
            f"{pad}  duration: {node.duration}",
            f"{pad}  impact: [{', '.join(str(x) for x in node.impact)}]",
        ]
    if node.kind == "xor" and node.xor_kind == "nature":
        head = [f"{pad}- nature: {round(node.probability, 6)}"]
    elif node.kind == "xor":
        head = [f"{pad}- choice:"]
    else:
        head = [f"{pad}- {node.kind}:"]
    return (
        head
        + [f"{pad}  id: {node.node_id}"]
        + [f"{pad}  low:"]
        + to_yaml(node.low, indent + 2)
        + [f"{pad}  high:"]
        + to_yaml(node.high, indent + 2)
    )


def instance_path(root: Path, key: dict, suffix: str) -> Path:
    """One directory per structural coordinate, the files of a process together.

    A flat grid is 5400 files in one directory, which no one can read and no
    shell can glob comfortably. Nested, the path names its own coordinates and
    everything computed on one process sits in one place:

        3-nested/2-independent/1-process_number/4-random.yaml
    """
    return (root
            / f"{key['nested']}-nested"
            / f"{key['independent']}-independent"
            / f"{key['process_number']}-process_number"
            / f"{key['dimensions']}-{key['mode']}{suffix}")


def write_process(key: dict, expression: str, tree: Node, out_dir: Path) -> Path:
    path = instance_path(out_dir, key, ".yaml")
    name = path.stem
    lines = [
        f"# {key['nested']}-{key['independent']}-{key['process_number']}"
        f"-{key['dimensions']}-{key['mode']}",
        f"nested: {key['nested']}",
        f"independent: {key['independent']}",
        f"process_number: {key['process_number']}",
        f"dimensions: {key['dimensions']}",
        f"mode: {key['mode']}",
        f"seed: {key['seed']}",
        f"tasks: {key['tasks']}",
        f"nodes: {key['nodes']}",
        f"max_duration: {key['max_duration']}",
        f"xor_root_kind: {key['xor_root_kind']}",
        f"source: generated_processes_full_{key['nested']}_{key['independent']}.txt"
        f" line {key['process_number']}",
        f"expression: \"{expression}\"",
        "tree:",
    ]
    lines += to_yaml(tree, indent=1)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines) + "\n")
    return path


# ---------------------------------------------------------------------------


def build(nested: int, independent: int, process_number: int, dimensions: int,
          mode: str, seed: int, out_dir: Path) -> Path:
    src = BENCH / "generated_processes" / f"generated_processes_full_{nested}_{independent}.txt"
    lines = [l for l in src.read_text().splitlines() if l.strip()]
    expression = lines[process_number - 1]

    # one seed per instance, derived from the run seed and the key through a
    # stable digest, so that a single file can be rebuilt without rebuilding
    # the grid and two runs of the same command write the same bytes on any
    # machine. The recorded campaign predates this derivation, its
    # generator having leaned on the hash of the interpreter run that made
    # it, so the grid this stage writes is drawn the same way and is not
    # that grid; the grid of the paper ships in default_experiment.
    digest = hashlib.sha256(
        f"{seed}|{nested}|{independent}|{process_number}|{dimensions}|{mode}".encode()
    ).digest()
    rng = random.Random(int.from_bytes(digest[:8], "big"))
    random.seed(rng.random())
    import numpy as np
    np.random.seed(rng.randrange(2**32))

    tree = parse(expression)
    assign_ids(tree, [0])
    root_kind = rng.choice(["choice", "nature"])
    assign_xor_kinds(tree, root_kind)
    draw_probabilities(tree, rng)
    max_duration = attach_impacts_and_durations(tree, dimensions, mode, rng)

    key = dict(nested=nested, independent=independent, process_number=process_number,
               dimensions=dimensions, mode=mode, seed=seed, tasks=len(tasks_of(tree)),
               nodes=len(nodes_of(tree)), xor_root_kind=root_kind,
               max_duration=max_duration)
    return write_process(key, expression, tree, out_dir)


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--nested", type=int, nargs="+", default=[3])
    p.add_argument("--independent", type=int, nargs="+", default=[2])
    p.add_argument("--process-number", type=int, nargs="+", default=[1])
    p.add_argument("--dimensions", type=int, nargs="+", default=[2],
                   help="2 to 10; dimension 1 degenerates into a shortest path")
    p.add_argument("--mode", nargs="+", default=["random"],
                   help="one or more of the six modes, or all")
    p.add_argument("--all", action="store_true",
                   help="the whole grid, nested and independent 1 to 10, ten processes each")
    p.add_argument("--seed", type=int, default=20260815)
    p.add_argument("--out", type=Path, default=OUT)
    a = p.parse_args()

    nested = range(1, 11) if a.all else a.nested
    independent = range(1, 11) if a.all else a.independent
    numbers = range(1, 11) if a.all else a.process_number
    modes = MODES if a.mode == ["all"] else a.mode

    for d in a.dimensions:
        if d < 2:
            p.error("dimensions start at 2: in dimension 1 the question is a shortest path")

    written = 0
    for n in nested:
        for i in independent:
            for k in numbers:
                for d in a.dimensions:
                    for m in modes:
                        if m not in MODES:
                            p.error(f"unknown mode {m}, expected one of {', '.join(MODES)}")
                        build(n, i, k, d, m, a.seed, a.out)
                        written += 1
    print(f"{written} trees written under {a.out}")


if __name__ == "__main__":
    main()
