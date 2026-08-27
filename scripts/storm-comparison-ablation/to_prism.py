#!/usr/bin/env python3
"""Translate one BPMN+CPI instance (YAML) into a PRISM model file.

The encoding follows Definitions 8 and 9 of ``algorithm-paper/semantics.tex``
clause by clause, from the non saturated semantics, so that every clause of the
transition relation becomes one guarded command of the model and no cascade of
moves is ever collapsed.

  state          one bounded integer variable per node of the process tree,
                 holding -1 for a node that has not started, -2 for a node that
                 is done, and the elapsed progress of a node that is running;
                 the initial value is 0 at the root and -1 everywhere else;
  triggers       one named formula per node and per trigger, so a guard reads
                 like the definition rather than like a wall of conjunctions;
  rank           ``rank_i`` conjoins the negation of every trigger of higher
                 priority, PRISM having no notion of priority between commands;
  witness        ``wit_i_v`` conjoins the negation of the same trigger at every
                 node of smaller identifier, the identifiers being the in-order
                 numbering the instance file carries;
  choice         two commands sharing a guard, one per branch, which is how a
                 decision is handed to the scheduler;
  nature         one command with a probabilistic update;
  down, up, ff   single deterministic commands;
  costs          k reward structures, one per component of the impact vector,
                 attached to the transitions that complete tasks.

Every place where the encoding departs from a literal reading of the definition
carries a comment, both here and in the emitted file. The departures are:

  1. Durations are rounded to integers after multiplication by a scale factor,
     because a PRISM variable is an integer while a duration of the instance is
     a real number. The factor is stated in the header of the emitted file: it
     changes what a time unit means.

  2. Definition 6 lets the progress of a node range over the whole of N. PRISM
     wants a bounded range, so each node is given a static upper bound on the
     time it can be running, computed by structural recursion over the tree
     (a task takes its duration, a sequence the sum of the two regions, a
     parallel or an XOR node the maximum of the two branches, since a parallel
     node advances both branches with the same t and an XOR node runs only one).

  3. Every arithmetic update is clamped with ``min``. PRISM builds the
     transition relation over the full cube of the declared ranges and not over
     the reachable states alone, and it rejects a model in which some valuation,
     reachable or not, sends an update out of range. The clamp is a no-op on
     every reachable state by the bound of point 2, so it does not change the
     transition relation of the semantics: it only gives the unreachable part of
     the cube a well defined value.

  4. The up clause of Definition 9 is one clause with two cases. The two cases
     are conditions on the source state that exclude one another, so they become
     two commands with disjoint guards, and only a sequence node receives both.

  5. The fast forward clause names a witness that its effect never mentions: the
     effect is global. One command per task is emitted all the same, guarded by
     the witness, so that commands and clause instances stay in bijection. The
     guards exclude one another, so at most one is enabled, and the resulting
     distributions would in any case merge in Steps(s).

  6. The cost of Definition 1 is one vector-valued function. PRISM carries named
     scalar reward structures, so the k components become k structures. They are
     transition rewards, not state rewards, since a multi-objective query over an
     MDP accepts transition rewards only. Each structure is a single reward item
     summing one conditional term per task, rather than one item per task,
     because the manual documents the summing of several matching items for
     state rewards and does not repeat the statement for transition rewards.

  7. The final state is given an explicit self loop, the absorption clause of
     Definition 9. Without it PRISM would report a deadlock and patch it with a
     self loop of its own.

Usage
-----
    python to_prism.py bpmn-cpi-benchmarks/3-2-1-4-random.yaml
    python to_prism.py 3-2-1-4-random
        reads  bpmn-cpi-benchmarks/3-nested/2-independent/1-process_number/4-random.yaml
        writes prism-benchmark-encodings/3-nested/2-independent/1-process_number/4-random.prism
    python to_prism.py instance.yaml --out -          # to standard output
    python to_prism.py 3-2-1-4-random --history
        writes prism-trail-encodings/3-nested/2-independent/1-process_number/4-random.prism
        the same model with the decision trail added, in its own tree
"""

from __future__ import annotations

import argparse
import sys
from dataclasses import dataclass, field
from pathlib import Path

import yaml

KINDS = ("task", "choice", "nature", "sequence", "parallel")
INTERNAL = ("choice", "nature", "sequence", "parallel")


# ---------------------------------------------------------------------------
# the instance, read
# ---------------------------------------------------------------------------


@dataclass
class Node:
    kind: str                       # task | choice | nature | sequence | parallel
    node_id: int                    # the in-order identifier of Definition 5
    name: str | None = None         # tasks only
    probability: float | None = None  # nature only, probability of the LOW branch
    raw_duration: float | None = None  # tasks only, as the instance gives it
    duration: int = 0               # tasks only, scaled and rounded
    impact: list[float] = field(default_factory=list)  # tasks only
    low: "Node | None" = None
    high: "Node | None" = None
    max_time: int = 0               # static bound on the progress of this node


def read_node(entry: dict, path: Path) -> Node:
    """One YAML mapping into one node.

    A node carries exactly one key naming its kind, plus its identifier, plus
    either a duration and an impact vector (a task) or a low and a high child.
    """
    kinds = [k for k in KINDS if k in entry]
    if len(kinds) != 1:
        raise ValueError(f"{path}: a node must name exactly one kind, found {kinds}")
    kind = kinds[0]
    node = Node(kind=kind, node_id=int(entry["id"]))
    if kind == "task":
        node.name = str(entry[kind])
        node.raw_duration = float(entry["duration"])
        node.impact = [float(x) for x in entry["impact"]]
        return node
    if kind == "nature":
        node.probability = float(entry[kind])
        if not 0.0 < node.probability < 1.0:
            raise ValueError(
                f"{path}: node {node.node_id} has probability {node.probability}, "
                "and Definition 3 asks for a probability in the open interval (0, 1)"
            )
    node.low = read_one(entry["low"], path)
    node.high = read_one(entry["high"], path)
    return node


def read_one(entries, path: Path) -> Node:
    """The instance writes every child as a list of one element."""
    if isinstance(entries, dict):
        return read_node(entries, path)
    if len(entries) != 1:
        raise ValueError(f"{path}: expected a single child, found {len(entries)}")
    return read_node(entries[0], path)


def nodes_of(node: Node) -> list[Node]:
    """In-order, so the list is sorted by identifier (Definition 5)."""
    if node.kind == "task":
        return [node]
    return nodes_of(node.low) + [node] + nodes_of(node.high)


def tasks_of(node: Node) -> list[Node]:
    if node.kind == "task":
        return [node]
    return tasks_of(node.low) + tasks_of(node.high)


def scale_durations(node: Node, scale: float, path: Path) -> None:
    for task in tasks_of(node):
        task.duration = int(round(task.raw_duration * scale))
        if task.duration < 1:
            raise ValueError(
                f"{path}: task {task.name} has duration {task.raw_duration}, which the "
                f"scale factor {scale} rounds to {task.duration}; Definition 2 asks for "
                "a duration of at least 1, so choose a larger factor"
            )


def compute_bounds(node: Node) -> int:
    """A static upper bound on the progress a node can reach while running.

    Departure 2 of the module docstring. The value of a node is the time elapsed
    since it started, so:

      task        its duration, the progress reaching D(v) only as the -2 that
                  the fast forward clause writes, hence a declared range that
                  stops at D(v) - 1, as well formedness demands;
      sequence    the low region runs to its end and the high region follows, so
                  the two bounds add;
      parallel    the fast forward clause advances both branches by the same t,
                  so the node closes with the slower of the two branches;
      XOR         exactly one branch ever runs.

    Every zero time move (choice, down, up, nature) has priority over the fast
    forward, so the moves that close a subtree all happen before time advances
    again, and a node never lingers past the completion of its own subtree.
    """
    if node.kind == "task":
        node.max_time = node.duration
        return node.max_time
    low = compute_bounds(node.low)
    high = compute_bounds(node.high)
    node.max_time = low + high if node.kind == "sequence" else max(low, high)
    return node.max_time


# ---------------------------------------------------------------------------
# formatting helpers
# ---------------------------------------------------------------------------


def dbl(x: float) -> str:
    """A PRISM double literal, never in exponent form."""
    text = f"{x:.12g}"
    if "e" in text or "E" in text:
        text = f"{x:.12f}".rstrip("0")
        if text.endswith("."):
            text += "0"
    if "." not in text:
        text += ".0"
    return text


def var(node: Node) -> str:
    return f"n{node.node_id}"


def label_of(node: Node) -> str:
    """How the node reads in a comment of the emitted file."""
    if node.kind == "task":
        return f"task {node.name}"
    if node.kind == "nature":
        return f"nature p={dbl(node.probability)}"
    return node.kind


# ---------------------------------------------------------------------------
# the triggers of Definition 8
# ---------------------------------------------------------------------------


def trigger(node: Node, i: int) -> str | None:
    """The body of Psi_i at this node, or None when Psi_i can never hold here.

    Transcribed from Definition 8. The conjunct L(v) = ... is decided at
    translation time and leaves no trace in the guard, the label of a node being
    fixed by the instance; what remains is the part that speaks about the state.
    """
    v, kind = var(node), node.kind
    if i == 1:  # choice: XOR outside Dom(P), running, both children idle
        if kind != "choice":
            return None
        return f"{v}>=0 & {var(node.low)}=-1 & {var(node.high)}=-1"
    if i == 2:  # down: parallel or sequence, running, both children idle
        if kind not in ("sequence", "parallel"):
            return None
        return f"{v}>=0 & {var(node.low)}=-1 & {var(node.high)}=-1"
    if i == 3:  # up: some child done at a sequence or an XOR, both at a parallel
        if kind == "task":
            return None
        if kind == "parallel":
            return f"{var(node.low)}=-2 & {var(node.high)}=-2"
        return f"{var(node.low)}=-2 | {var(node.high)}=-2"
    if i == 4:  # nature: XOR inside Dom(P), running, both children idle
        if kind != "nature":
            return None
        return f"{v}>=0 & {var(node.low)}=-1 & {var(node.high)}=-1"
    if i == 5:  # fast forward: a task under way
        if kind != "task":
            return None
        # v < D(v) is implied by the declared range of the variable; it is
        # emitted all the same, so that the guard reads as Definition 8 writes it
        return f"{v}>=0 & {v}<d{node.node_id}"
    raise AssertionError(i)


# ---------------------------------------------------------------------------
# the emitter
# ---------------------------------------------------------------------------


def emit(data: dict, root: Node, source: Path, scale: float,
         history: bool = False) -> str:
    nodes = nodes_of(root)
    tasks = tasks_of(root)
    dimension = len(tasks[0].impact)
    for task in tasks:
        if len(task.impact) != dimension:
            raise ValueError(f"{source}: impact vectors of different lengths")
    holders = {i: [n for n in nodes if trigger(n, i) is not None] for i in range(1, 6)}
    out: list[str] = []
    w = out.append

    # -- header ------------------------------------------------------------
    key = ("nested", "independent", "process_number", "dimensions", "mode", "seed",
           "tasks", "nodes", "max_duration", "xor_root_kind")
    w(f"// PRISM model of the BPMN+CPI instance {source.stem}")
    w(f"// generated by to_prism.py from {source.name}")
    w("//")
    w("// instance key")
    for k in key:
        if k in data:
            w(f"//   {k}: {data[k]}")
    if "expression" in data:
        w(f"//   expression: {data['expression']}")
    w("//")
    w("// TIME SCALING")
    w(f"//   Every duration of the instance is multiplied by {dbl(scale)} and rounded to")
    w("//   the nearest integer, a PRISM variable being an integer while a duration of")
    w("//   the instance is a real number. One time unit of this model is therefore")
    w(f"//   {dbl(1.0 / scale)} time units of the instance, and every progress value, every")
    w("//   duration constant and every variable bound below is read in the scaled unit.")
    w("//   The impact vectors are not scaled: they are carried unchanged as doubles.")
    w("//")
    w("// HOW TO CHECK THIS MODEL")
    w('//   prism <model> -pf \'R{"impact0"}min=? [ F "final" ]\' -explicit')
    w("//   prism <model> -pf 'multi(R{\"impact0\"}min=? [ C ], R{\"impact1\"}<=b [ C ])'")
    w("//   The explicit engine builds the reachable states alone. Every other engine")
    w("//   (the default one, sparse and hybrid) builds the transition relation")
    w("//   symbolically, over the FULL cube of the declared ranges, so a large scale")
    w("//   factor widens every range and exhausts CUDD long before the reachable part")
    w("//   of the model becomes large. At scale 1 every engine builds this model.")
    w("//")
    w("// The encoding follows Definitions 8 and 9 of semantics.tex clause by clause.")
    w("// Comments below mark the places where it departs from a literal reading.")
    w("")
    w("mdp")
    w("")

    # -- constants ---------------------------------------------------------
    w("// ---------------------------------------------------------------------------")
    w("// durations, scaled and rounded (D of Definition 3)")
    w("// ---------------------------------------------------------------------------")
    for task in tasks:
        w(f"const int d{task.node_id} = {task.duration};"
          f"  // {task.name}, raw duration {dbl(task.raw_duration)}")
    horizon = max(task.duration for task in tasks) + 1
    w("")
    w("// larger than any remaining time, so that a task that is not running never wins")
    w("// the minimum of the fast forward clause")
    w(f"const int no_time = {horizon};")
    natures = [n for n in nodes if n.kind == "nature"]
    if natures:
        w("")
        w("// ---------------------------------------------------------------------------")
        w("// nature probabilities (P_phi of Definition 3), each one the probability of")
        w("// the LOW branch")
        w("// ---------------------------------------------------------------------------")
        for n in natures:
            w(f"const double p{n.node_id} = {dbl(n.probability)};")
    w("")

    # -- triggers ----------------------------------------------------------
    w("// ---------------------------------------------------------------------------")
    w("// the five triggers of Definition 8, one formula per node and per trigger.")
    w("// The conjunct on the label of the node is decided here and leaves no trace:")
    w("// a formula is emitted only for a node whose label lets the trigger hold.")
    w("// ---------------------------------------------------------------------------")
    names = {1: "choice", 2: "down", 3: "up", 4: "nature", 5: "fast forward"}
    for i in range(1, 6):
        w(f"// Psi_{i}, the {names[i]} trigger")
        for n in holders[i]:
            w(f"formula psi{i}_{n.node_id} = {trigger(n, i)};"
              f"  // node {n.node_id}, {label_of(n)}")
        body = " | ".join(f"psi{i}_{n.node_id}" for n in holders[i]) or "false"
        w(f"formula some_psi{i} = {body};")
        w("")

    # -- rank --------------------------------------------------------------
    w("// ---------------------------------------------------------------------------")
    w("// the rank r(s) of Definition 8. DEPARTURE: PRISM offers every command whose")
    w("// guard holds and has no notion of priority, so the priority order of the")
    w("// triggers is written by hand, each rank negating the ranks above it.")
    w("// ---------------------------------------------------------------------------")
    for i in range(1, 6):
        conj = [f"!some_psi{j}" for j in range(1, i)] + [f"some_psi{i}"]
        w(f"formula rank{i} = {' & '.join(conj)};")
    w("")

    # -- witness -----------------------------------------------------------
    w("// ---------------------------------------------------------------------------")
    w("// the witness w(s) of Definition 8, the node of LEAST identifier among those")
    w("// satisfying the trigger of the rank. DEPARTURE: the argmin is written by hand")
    w("// as well, by negating the same trigger at every node of smaller identifier.")
    w("// The identifiers are fixed by the instance, so these conjuncts are static.")
    w("// ---------------------------------------------------------------------------")
    for i in range(1, 6):
        for pos, n in enumerate(holders[i]):
            smaller = [f"!psi{i}_{m.node_id}" for m in holders[i][:pos]]
            body = " & ".join([f"psi{i}_{n.node_id}"] + smaller)
            w(f"formula wit{i}_{n.node_id} = {body};")
        if holders[i]:
            w("")

    # -- fast forward arithmetic -------------------------------------------
    w("// ---------------------------------------------------------------------------")
    w("// the fast forward clause of Definition 9: the step t is the least remaining")
    w("// time over the running tasks, and the closers are the tasks that reach it.")
    w("// ---------------------------------------------------------------------------")
    remaining = [f"{var(t)}>=0 ? d{t.node_id}-{var(t)} : no_time" for t in tasks]
    if len(remaining) == 1:
        w(f"formula t_step = {remaining[0]};")
    else:
        w("formula t_step = min(" + ",\n                     ".join(remaining) + ");")
    w("")
    for task in tasks:
        w(f"formula closes{task.node_id} = "
          f"{var(task)}>=0 & d{task.node_id}-{var(task)}=t_step;  // {task.name}")
    w("")
    w("// the successor of the fast forward clause, given pointwise: a closer is")
    w("// completed, every other running node advances by t, and a node that is not")
    w("// running keeps its value.")
    w("// DEPARTURE: every sum is clamped with min. PRISM builds the transition relation")
    w("// over the full cube of the declared ranges and not over the reachable states")
    w("// alone, and it rejects a model in which some valuation, reachable or not, sends")
    w("// an update out of range. The clamp is a no-op on every reachable state, by the")
    w("// bound each variable carries, so the transition relation of the semantics is")
    w("// untouched: it only gives the unreachable part of the cube a defined value.")
    for n in nodes:
        u = var(n)
        if n.kind == "task":
            body = (f"{u}<0 ? {u} : "
                    f"(closes{n.node_id} ? -2 : min({u}+t_step, {n.duration - 1}))")
        else:
            body = f"{u}<0 ? {u} : min({u}+t_step, {n.max_time})"
        w(f"formula ff{n.node_id} = {body};  // node {n.node_id}, {label_of(n)}")
    w("")

    # -- module ------------------------------------------------------------
    w("// ---------------------------------------------------------------------------")
    w("// the state of Definition 6: one variable per node, -1 idle, -2 done, and a")
    w("// non-negative value the elapsed progress. The initial state s_0 puts the root")
    w("// at 0 and every other node at -1.")
    w("// DEPARTURE: Definition 6 lets the progress range over the whole of N. A PRISM")
    w("// variable is bounded, so each node carries a static bound on the time it can")
    w("// be running: a task stops one below its duration, as well formedness demands,")
    w("// a sequence adds the bounds of its two regions, and a parallel or an XOR node")
    w("// takes the larger of the two branches.")
    w("// ---------------------------------------------------------------------------")
    w("module process")
    w("")
    for n in nodes:
        top = n.duration - 1 if n.kind == "task" else n.max_time
        init = 0 if n is root else -1
        w(f"  {var(n)} : [-2..{top}] init {init};"
          f"  // node {n.node_id}, {label_of(n)}")
    w("")

    # -- the decision trail, under --history --------------------------------
    # The up clause returns the branches of a closed region to the idle value,
    # so a state does not say which branch was taken, and two runs that decided
    # a choice differently meet again. That is sound for the value of a policy
    # and wrong for the class of policies: a memoryless policy on this model
    # cannot react to a decision already closed, while a policy of ours, which
    # reads a history, can. Under --history each choice and each nature node
    # carries a variable recording the branch it took, written once and never
    # cleared. Everything else is untouched, guards included, so the model is
    # the same modulo a labelling of the runs by their decisions: the values
    # and the probabilities do not move, only states that differ in a past
    # decision stop being identified. A memoryless deterministic policy on the
    # result is a deterministic policy of ours on the original, since a run is
    # determined by its decisions and a state records how far it has gone.
    trail = [n for n in nodes if n.kind in ("choice", "nature")] if history else []
    if trail:
        w("  // the decision trail: 0 not yet decided, 1 the low branch, 2 the high one")
        for n in trail:
            w(f"  d{n.node_id} : [0..2] init 0;  // node {n.node_id}, {label_of(n)}")
        w("")

    def mark(n: Node, branch: int) -> str:
        return f" & (d{n.node_id}'={branch})" if n in trail else ""

    # (choice)
    if holders[1]:
        w("  // ---- (choice), r(s) = 1 ------------------------------------------------")
        w("  // A(s) = {low(v), high(v)}: two commands sharing a guard, which is how a")
        w("  // decision is handed to the scheduler. The two effects differ, so the two")
        w("  // distributions do not merge in Steps(s).")
        for n in holders[1]:
            g = f"rank1 & wit1_{n.node_id}"
            w(f"  [choice{n.node_id}low]  {g} -> ({var(n.low)}'=0){mark(n, 1)};")
            w(f"  [choice{n.node_id}high] {g} -> ({var(n.high)}'=0){mark(n, 2)};")
        w("")

    # (down)
    if holders[2]:
        w("  // ---- (down), r(s) = 2 --------------------------------------------------")
        w("  // tau starts the low child of a sequence and both children of a parallel.")
        for n in holders[2]:
            g = f"rank2 & wit2_{n.node_id}"
            if n.kind == "sequence":
                upd = f"({var(n.low)}'=0)"
            else:
                upd = f"({var(n.low)}'=0) & ({var(n.high)}'=0)"
            w(f"  [down{n.node_id}] {g} -> {upd};  // {n.kind}")
        w("")

    # (up)
    if holders[3]:
        w("  // ---- (up), r(s) = 3 ----------------------------------------------------")
        w("  // DEPARTURE: the clause has two cases. They are conditions on the source")
        w("  // state that exclude one another, so they become two commands with")
        w("  // disjoint guards, and only a sequence node receives both. The first hands")
        w("  // a sequence over from its first region to its second; the second closes")
        w("  // the node above the branches that are done.")
        for n in holders[3]:
            g = f"rank3 & wit3_{n.node_id}"
            lo, hi, me = var(n.low), var(n.high), var(n)
            if n.kind == "sequence":
                hand = f"{lo}=-2 & {hi}=-1"
                w(f"  [up{n.node_id}hand]  {g} & ({hand}) -> ({lo}'=-1) & ({hi}'=0);")
                w(f"  [up{n.node_id}close] {g} & !({hand}) -> "
                  f"({lo}'=-1) & ({hi}'=-1) & ({me}'=-2);")
            else:
                w(f"  [up{n.node_id}close] {g} -> "
                  f"({lo}'=-1) & ({hi}'=-1) & ({me}'=-2);  // {n.kind}")
        w("")

    # (nature)
    if holders[4]:
        w("  // ---- (nature), r(s) = 4 ------------------------------------------------")
        w("  // one command with a probabilistic update, the low branch carrying p.")
        for n in holders[4]:
            g = f"rank4 & wit4_{n.node_id}"
            w(f"  [nature{n.node_id}] {g} -> "
              f"p{n.node_id} : ({var(n.low)}'=0){mark(n, 1)} + "
              f"(1-p{n.node_id}) : ({var(n.high)}'=0){mark(n, 2)};")
        w("")

    # (fast forward)
    w("  // ---- (fast forward), r(s) = 5 ------------------------------------------")
    w("  // The clause completes every closer and advances every other running node by")
    w("  // t, tasks and internal nodes alike; the successor is given pointwise, so")
    w("  // every node is assigned.")
    w("  // DEPARTURE: the effect never mentions the witness, the clause being global.")
    w("  // One command per task is emitted all the same, so that commands and clause")
    w("  // instances stay in bijection; the guards exclude one another, so at most one")
    w("  // is enabled, and identical distributions would in any case merge in Steps(s).")
    updates = " & ".join(f"({var(n)}'=ff{n.node_id})" for n in nodes)
    for task in tasks:
        w(f"  [ff] rank5 & wit5_{task.node_id} -> {updates};")
    w("")

    # (absorption)
    w("  // ---- (absorption) ------------------------------------------------------")
    w("  // s = s_F, the final state, where no trigger holds. DEPARTURE: the self loop")
    w("  // is written out, since PRISM would otherwise report a deadlock and patch it.")
    w(f"  [absorb] {var(root)}=-2 -> ({var(root)}'=-2);")
    w("")
    w("endmodule")
    w("")
    w("// the final state of Definition 7: the root is done")
    w(f'label "final" = {var(root)}=-2;')
    w("")

    # -- rewards -----------------------------------------------------------
    w("// ---------------------------------------------------------------------------")
    w("// the cost of Definition 3, one vector-valued function, becomes k reward")
    w("// structures, k being the dimension of the instance. They are TRANSITION")
    w("// rewards, attached to the fast forward transitions, which are the only ones")
    w("// that pay: a multi-objective query over an MDP accepts transition rewards")
    w("// only. Each structure is a single item summing one conditional term per task,")
    w("// rather than one item per task, because the manual documents the summing of")
    w("// several matching items for state rewards and does not repeat the statement")
    w("// for transition rewards. The guard of an item is read in the SOURCE state,")
    w("// which is where Closers(s) is read as well.")
    w("// ---------------------------------------------------------------------------")
    for j in range(dimension):
        w(f'rewards "impact{j}"')
        w("  [ff] true :")
        for pos, t in enumerate(tasks):
            plus = "  " if pos == 0 else "+ "
            w(f"      {plus}(closes{t.node_id} ? {dbl(t.impact[j])} : 0.0)   // {t.name}")
        # the semicolon sits on a line of its own: a // comment runs to the end of
        # the line, so a semicolon placed after the last comment would be eaten
        w("  ;")
        w("endrewards")
        w("")
    return "\n".join(out) + "\n"


# ---------------------------------------------------------------------------


INSTANCES = Path(__file__).resolve().parent / "bpmn-cpi-benchmarks"
ENCODINGS = Path(__file__).resolve().parent / "prism-benchmark-encodings"
# the augmented models live in their own tree, at the same coordinates: the two
# encodings answer different questions, the plain one every question about a
# single component, where a memoryless policy is already optimal, the augmented
# one the multi-objective question over our class of policies, and neither
# replaces the other
TRAIL_ENCODINGS = Path(__file__).resolve().parent / "prism-trail-encodings"


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("instance",
                   help="the key of an instance, for example 3-2-1-4-random, or a "
                        "path to a yaml file. A key is looked up in "
                        "bpmn-cpi-benchmarks and its model is written under the "
                        "same name to prism-benchmark-encodings, or to "
                        "prism-trail-encodings under --history")
    p.add_argument("--scale", type=float, default=1.0,
                   help="multiply every duration by this factor and round "
                        "(default 1, the instances carrying integer durations "
                        "already); a PRISM variable is an integer, so a instance "
                        "with real durations needs a factor here")
    p.add_argument("--out", type=Path, default=None,
                   help="write the model here instead of the default place; "
                        "- is stdout")
    p.add_argument("--history", action="store_true",
                   help="record in the state the branch taken at every choice "
                        "and every nature node, so that runs which decided "
                        "differently are never identified. The values of the "
                        "model do not change; what changes is that a memoryless "
                        "policy on it is a history-dependent policy on the "
                        "original, which is the class our decision problem "
                        "quantifies over and the class storm can be restricted "
                        "to with --multiobjective:purescheds positional")
    a = p.parse_args()

    root_dir = TRAIL_ENCODINGS if a.history else ENCODINGS
    given = Path(a.instance)
    if given.suffix == ".yaml" or given.exists():
        a.yaml = given
        # mirror the layout of the instances under the encodings, so that the
        # model of an instance sits at the same coordinates as the instance
        try:
            rel = given.resolve().relative_to(INSTANCES)
            default_out = (root_dir / rel).with_suffix(".prism")
        except ValueError:
            default_out = root_dir / (given.stem + ".prism")
    else:
        try:
            nested, independent, number, dimensions, mode = a.instance.split("-", 4)
        except ValueError:
            p.error(f"a key reads nested-independent-process_number-dimensions-mode, "
                    f"as in 3-2-1-4-random; got {a.instance}")
        rel = (Path(f"{nested}-nested") / f"{independent}-independent"
               / f"{number}-process_number" / f"{dimensions}-{mode}")
        a.yaml = (INSTANCES / rel).with_suffix(".yaml")
        default_out = (root_dir / rel).with_suffix(".prism")
        if not a.yaml.exists():
            p.error(f"no instance at {a.yaml}; build it with make_bpmn_cpi.py")

    if a.scale <= 0:
        p.error("the scale factor must be positive")

    data = yaml.safe_load(a.yaml.read_text())
    root = read_one(data["tree"], a.yaml)
    scale_durations(root, a.scale, a.yaml)
    compute_bounds(root)

    ids = [n.node_id for n in nodes_of(root)]
    if ids != sorted(ids) or ids != list(range(1, len(ids) + 1)):
        raise ValueError(
            f"{a.yaml}: the identifiers are not the in-order numbering 1..|V| of "
            f"Definition 6, found {ids}"
        )

    text = emit(data, root, a.yaml, a.scale, a.history)
    out = a.out if a.out is not None else default_out
    if str(out) == "-":
        sys.stdout.write(text)
    else:
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(text)
        tasks = tasks_of(root)
        print(f"{out}: {len(nodes_of(root))} nodes, {len(tasks)} tasks, "
              f"dimension {len(tasks[0].impact)}, scale {a.scale:g}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
