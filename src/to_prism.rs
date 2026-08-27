//! One BPMN+CPI instance into a PRISM model, mdp, following the non
//! saturated semantics clause by clause: one bounded integer variable per
//! node (-1 idle, -2 done, otherwise the elapsed progress), one named
//! formula per node and per trigger, the priority between the triggers and
//! the least-identifier witness written by hand as conjunctions of
//! negations, a choice as two commands sharing a guard, a nature node as
//! one probabilistic command, the fast forward as a global pointwise
//! update, and k transition reward structures, one per component of the
//! impact vector, attached to the fast forward.
//!
//! With the history encoding, on by default, each choice and each nature
//! node carries one extra variable recording the branch it took, written
//! once and never cleared. The plain model returns the branches of a
//! closed region to the idle value, so two runs that decided a choice
//! differently meet again, and a memoryless policy on it cannot react to a
//! decision already closed; with the trail the runs stop being identified,
//! the values and the probabilities do not move, and a memoryless
//! deterministic policy on the result is a history-dependent deterministic
//! policy on the original, the class the decision problem quantifies over
//! and the class a model checker can be restricted to with a positional
//! pure scheduler. Without the trail the model answers every question
//! about a single component, where a memoryless policy is already optimal;
//! neither encoding replaces the other.
//!
//! The emitted text matches scripts/to_prism.py line for line at scale 1,
//! apart from the one header line naming the generator, so the two
//! encoders can be checked against one another with a byte comparison.

use crate::tree::{Kind, Tree};

fn kind_str(k: Kind) -> &'static str {
    match k {
        Kind::Task => "task",
        Kind::Sequence => "sequence",
        Kind::Parallel => "parallel",
        Kind::Choice => "choice",
        Kind::Nature => "nature",
    }
}

/// `%.12g` without the exponent form, matching `dbl` of scripts/to_prism.py:
/// a PRISM double literal.
fn dbl(x: f64) -> String {
    let mut t = format_g(x, 12);
    if t.contains('e') || t.contains('E') {
        t = format!("{x:.12}");
        while t.ends_with('0') {
            t.pop();
        }
        if t.ends_with('.') {
            t.push('0');
        }
    }
    if !t.contains('.') {
        t.push_str(".0");
    }
    t
}

/// The `%g` of C at `sig` significant digits: fixed notation with the
/// trailing zeros stripped when the exponent allows it, the exponent form
/// otherwise.
fn format_g(x: f64, sig: usize) -> String {
    if x == 0.0 {
        return "0".to_string();
    }
    let e = format!("{:.*e}", sig - 1, x);
    let (_, exp) = e.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    if exp < -4 || exp >= sig as i32 {
        return e;
    }
    let prec = (sig as i32 - 1 - exp).max(0) as usize;
    let mut t = format!("{:.*}", prec, x);
    if t.contains('.') {
        while t.ends_with('0') {
            t.pop();
        }
        if t.ends_with('.') {
            t.pop();
        }
    }
    t
}

/// A static upper bound on the progress a node can reach while running: a
/// task its duration, a sequence the sum of its two regions, a parallel or
/// an XOR node the larger of the two branches.
fn compute_bounds(tree: &Tree, id: u32, max_time: &mut Vec<i64>) -> i64 {
    let n = tree.node(id);
    let b = if n.kind == Kind::Task {
        n.duration as i64
    } else {
        let low = compute_bounds(tree, n.low, max_time);
        let high = compute_bounds(tree, n.high, max_time);
        if n.kind == Kind::Sequence {
            low + high
        } else {
            low.max(high)
        }
    };
    max_time[id as usize] = b;
    b
}

/// The body of `Psi_i` at this node, or `None` where the label of the node
/// never lets the trigger hold. The conjunct on the label is decided here
/// and leaves no trace in the guard.
fn trigger(tree: &Tree, id: u32, i: usize) -> Option<String> {
    let n = tree.node(id);
    let v = format!("n{id}");
    match i {
        // choice: XOR outside Dom(P), running, both children idle
        1 => {
            if n.kind != Kind::Choice {
                return None;
            }
            Some(format!("{v}>=0 & n{}=-1 & n{}=-1", n.low, n.high))
        }
        // down: parallel or sequence, running, both children idle
        2 => {
            if !matches!(n.kind, Kind::Sequence | Kind::Parallel) {
                return None;
            }
            Some(format!("{v}>=0 & n{}=-1 & n{}=-1", n.low, n.high))
        }
        // up: some child done at a sequence or an XOR, both at a parallel
        3 => {
            if n.kind == Kind::Task {
                return None;
            }
            if n.kind == Kind::Parallel {
                Some(format!("n{}=-2 & n{}=-2", n.low, n.high))
            } else {
                Some(format!("n{}=-2 | n{}=-2", n.low, n.high))
            }
        }
        // nature: XOR inside Dom(P), running, both children idle
        4 => {
            if n.kind != Kind::Nature {
                return None;
            }
            Some(format!("{v}>=0 & n{}=-1 & n{}=-1", n.low, n.high))
        }
        // fast forward: a task under way; v < D(v) is implied by the range
        // of the variable and emitted all the same, so that the guard reads
        // as the definition writes it
        5 => {
            if n.kind != Kind::Task {
                return None;
            }
            Some(format!("{v}>=0 & {v}<d{id}"))
        }
        _ => unreachable!(),
    }
}

/// How the node reads in a comment of the emitted file.
fn label_of(tree: &Tree, id: u32) -> String {
    let n = tree.node(id);
    match n.kind {
        Kind::Task => format!("task {}", tree.task_names[id as usize]),
        Kind::Nature => format!("nature p={}", dbl(n.prob)),
        k => kind_str(k).to_string(),
    }
}

pub fn emit(tree: &Tree, stem: &str, file_name: &str, history: bool) -> String {
    let nodes: Vec<u32> = (1..=tree.n_nodes).collect();
    let tasks: Vec<u32> = tree.tasks.clone();
    let dimension = tree.k;
    let mut max_time = vec![0i64; tree.n_nodes as usize + 1];
    compute_bounds(tree, tree.root, &mut max_time);
    let holders: Vec<Vec<u32>> = (1..=5)
        .map(|i| {
            nodes
                .iter()
                .copied()
                .filter(|&id| trigger(tree, id, i).is_some())
                .collect()
        })
        .collect();
    let holder = |i: usize| -> &Vec<u32> { &holders[i - 1] };
    let mut out: Vec<String> = Vec::new();
    let mut w = |s: String| out.push(s);
    let ws = |s: &str| s.to_string();
    let m = &tree.meta;

    // -- header ------------------------------------------------------------
    w(format!("// PRISM model of the BPMN+CPI instance {stem}"));
    w(format!("// generated by sdcpi to_prism from {file_name}"));
    w(ws("//"));
    w(ws("// instance key"));
    // the keys of the instance header, in the order to_prism.py prints them,
    // the grid keys only where the instance came from the grid
    let grid = !m.mode.is_empty();
    if grid {
        w(format!("//   nested: {}", m.nested));
        w(format!("//   independent: {}", m.independent));
        w(format!("//   process_number: {}", m.process_number));
    }
    w(format!("//   dimensions: {}", dimension));
    if grid {
        w(format!("//   mode: {}", m.mode));
        w(format!("//   seed: {}", m.seed));
    }
    w(format!("//   tasks: {}", tree.n_tasks));
    w(format!("//   nodes: {}", tree.n_nodes));
    if grid {
        w(format!("//   max_duration: {}", m.max_duration));
        w(format!("//   xor_root_kind: {}", m.xor_root_kind));
    }
    if !m.expression.is_empty() {
        w(format!("//   expression: {}", m.expression));
    }
    w(ws("//"));
    w(ws("// TIME SCALING"));
    w(ws("//   Every duration of the instance is multiplied by 1.0 and rounded to"));
    w(ws("//   the nearest integer, a PRISM variable being an integer while a duration of"));
    w(ws("//   the instance is a real number. One time unit of this model is therefore"));
    w(ws("//   1.0 time units of the instance, and every progress value, every"));
    w(ws("//   duration constant and every variable bound below is read in the scaled unit."));
    w(ws("//   The impact vectors are not scaled: they are carried unchanged as doubles."));
    w(ws("//"));
    w(ws("// HOW TO CHECK THIS MODEL"));
    w(ws("//   prism <model> -pf 'R{\"impact0\"}min=? [ F \"final\" ]' -explicit"));
    w(ws("//   prism <model> -pf 'multi(R{\"impact0\"}min=? [ C ], R{\"impact1\"}<=b [ C ])'"));
    w(ws("//   The explicit engine builds the reachable states alone. Every other engine"));
    w(ws("//   (the default one, sparse and hybrid) builds the transition relation"));
    w(ws("//   symbolically, over the FULL cube of the declared ranges, so a large scale"));
    w(ws("//   factor widens every range and exhausts CUDD long before the reachable part"));
    w(ws("//   of the model becomes large. At scale 1 every engine builds this model."));
    w(ws("//"));
    w(ws("// The encoding follows Definitions 8 and 9 of semantics.tex clause by clause."));
    w(ws("// Comments below mark the places where it departs from a literal reading."));
    w(ws(""));
    w(ws("mdp"));
    w(ws(""));

    // -- constants ---------------------------------------------------------
    w(ws("// ---------------------------------------------------------------------------"));
    w(ws("// durations, scaled and rounded (D of Definition 3)"));
    w(ws("// ---------------------------------------------------------------------------"));
    for &t in &tasks {
        let d = tree.node(t).duration;
        w(format!(
            "const int d{t} = {d};  // {}, raw duration {}",
            tree.task_names[t as usize],
            dbl(d as f64)
        ));
    }
    let horizon = tasks
        .iter()
        .map(|&t| tree.node(t).duration as i64)
        .max()
        .unwrap_or(0)
        + 1;
    w(ws(""));
    w(ws("// larger than any remaining time, so that a task that is not running never wins"));
    w(ws("// the minimum of the fast forward clause"));
    w(format!("const int no_time = {horizon};"));
    let natures: Vec<u32> = nodes
        .iter()
        .copied()
        .filter(|&id| tree.kind(id) == Kind::Nature)
        .collect();
    if !natures.is_empty() {
        w(ws(""));
        w(ws("// ---------------------------------------------------------------------------"));
        w(ws("// nature probabilities (P_phi of Definition 3), each one the probability of"));
        w(ws("// the LOW branch"));
        w(ws("// ---------------------------------------------------------------------------"));
        for &n in &natures {
            w(format!("const double p{n} = {};", dbl(tree.node(n).prob)));
        }
    }
    w(ws(""));

    // -- triggers ----------------------------------------------------------
    w(ws("// ---------------------------------------------------------------------------"));
    w(ws("// the five triggers of Definition 8, one formula per node and per trigger."));
    w(ws("// The conjunct on the label of the node is decided here and leaves no trace:"));
    w(ws("// a formula is emitted only for a node whose label lets the trigger hold."));
    w(ws("// ---------------------------------------------------------------------------"));
    let names = ["choice", "down", "up", "nature", "fast forward"];
    for i in 1..=5usize {
        w(format!("// Psi_{i}, the {} trigger", names[i - 1]));
        for &n in holder(i) {
            w(format!(
                "formula psi{i}_{n} = {};  // node {n}, {}",
                trigger(tree, n, i).unwrap(),
                label_of(tree, n)
            ));
        }
        let body = holder(i)
            .iter()
            .map(|n| format!("psi{i}_{n}"))
            .collect::<Vec<_>>()
            .join(" | ");
        let body = if body.is_empty() { "false".to_string() } else { body };
        w(format!("formula some_psi{i} = {body};"));
        w(ws(""));
    }

    // -- rank --------------------------------------------------------------
    w(ws("// ---------------------------------------------------------------------------"));
    w(ws("// the rank r(s) of Definition 8. DEPARTURE: PRISM offers every command whose"));
    w(ws("// guard holds and has no notion of priority, so the priority order of the"));
    w(ws("// triggers is written by hand, each rank negating the ranks above it."));
    w(ws("// ---------------------------------------------------------------------------"));
    for i in 1..=5usize {
        let mut conj: Vec<String> = (1..i).map(|j| format!("!some_psi{j}")).collect();
        conj.push(format!("some_psi{i}"));
        w(format!("formula rank{i} = {};", conj.join(" & ")));
    }
    w(ws(""));

    // -- witness -----------------------------------------------------------
    w(ws("// ---------------------------------------------------------------------------"));
    w(ws("// the witness w(s) of Definition 8, the node of LEAST identifier among those"));
    w(ws("// satisfying the trigger of the rank. DEPARTURE: the argmin is written by hand"));
    w(ws("// as well, by negating the same trigger at every node of smaller identifier."));
    w(ws("// The identifiers are fixed by the instance, so these conjuncts are static."));
    w(ws("// ---------------------------------------------------------------------------"));
    for i in 1..=5usize {
        for (pos, &n) in holder(i).iter().enumerate() {
            let mut body = vec![format!("psi{i}_{n}")];
            for &m in &holder(i)[..pos] {
                body.push(format!("!psi{i}_{m}"));
            }
            w(format!("formula wit{i}_{n} = {};", body.join(" & ")));
        }
        if !holder(i).is_empty() {
            w(ws(""));
        }
    }

    // -- fast forward arithmetic -------------------------------------------
    w(ws("// ---------------------------------------------------------------------------"));
    w(ws("// the fast forward clause of Definition 9: the step t is the least remaining"));
    w(ws("// time over the running tasks, and the closers are the tasks that reach it."));
    w(ws("// ---------------------------------------------------------------------------"));
    let remaining: Vec<String> = tasks
        .iter()
        .map(|&t| format!("n{t}>=0 ? d{t}-n{t} : no_time"))
        .collect();
    if remaining.len() == 1 {
        w(format!("formula t_step = {};", remaining[0]));
    } else {
        w(format!(
            "formula t_step = min({});",
            remaining.join(",\n                     ")
        ));
    }
    w(ws(""));
    for &t in &tasks {
        w(format!(
            "formula closes{t} = n{t}>=0 & d{t}-n{t}=t_step;  // {}",
            tree.task_names[t as usize]
        ));
    }
    w(ws(""));
    w(ws("// the successor of the fast forward clause, given pointwise: a closer is"));
    w(ws("// completed, every other running node advances by t, and a node that is not"));
    w(ws("// running keeps its value."));
    w(ws("// DEPARTURE: every sum is clamped with min. PRISM builds the transition relation"));
    w(ws("// over the full cube of the declared ranges and not over the reachable states"));
    w(ws("// alone, and it rejects a model in which some valuation, reachable or not, sends"));
    w(ws("// an update out of range. The clamp is a no-op on every reachable state, by the"));
    w(ws("// bound each variable carries, so the transition relation of the semantics is"));
    w(ws("// untouched: it only gives the unreachable part of the cube a defined value."));
    for &n in &nodes {
        let u = format!("n{n}");
        let body = if tree.kind(n) == Kind::Task {
            format!(
                "{u}<0 ? {u} : (closes{n} ? -2 : min({u}+t_step, {}))",
                tree.node(n).duration as i64 - 1
            )
        } else {
            format!("{u}<0 ? {u} : min({u}+t_step, {})", max_time[n as usize])
        };
        w(format!("formula ff{n} = {body};  // node {n}, {}", label_of(tree, n)));
    }
    w(ws(""));

    // -- module ------------------------------------------------------------
    w(ws("// ---------------------------------------------------------------------------"));
    w(ws("// the state of Definition 6: one variable per node, -1 idle, -2 done, and a"));
    w(ws("// non-negative value the elapsed progress. The initial state s_0 puts the root"));
    w(ws("// at 0 and every other node at -1."));
    w(ws("// DEPARTURE: Definition 6 lets the progress range over the whole of N. A PRISM"));
    w(ws("// variable is bounded, so each node carries a static bound on the time it can"));
    w(ws("// be running: a task stops one below its duration, as well formedness demands,"));
    w(ws("// a sequence adds the bounds of its two regions, and a parallel or an XOR node"));
    w(ws("// takes the larger of the two branches."));
    w(ws("// ---------------------------------------------------------------------------"));
    w(ws("module process"));
    w(ws(""));
    for &n in &nodes {
        let top = if tree.kind(n) == Kind::Task {
            tree.node(n).duration as i64 - 1
        } else {
            max_time[n as usize]
        };
        let init = if n == tree.root { 0 } else { -1 };
        w(format!(
            "  n{n} : [-2..{top}] init {init};  // node {n}, {}",
            label_of(tree, n)
        ));
    }
    w(ws(""));

    // The decision trail. The up clause returns the branches of a closed
    // region to the idle value, so a state does not say which branch was
    // taken, and two runs that decided a choice differently meet again.
    // That is sound for the value of a policy and wrong for the class of
    // policies: a memoryless policy on the plain model cannot react to a
    // decision already closed, while a history-dependent one can. With the
    // trail each choice and each nature node carries a variable recording
    // the branch it took, written once and never cleared; guards are
    // untouched, so the values and the probabilities do not move, and only
    // states that differ in a past decision stop being identified.
    let trail: Vec<u32> = if history {
        nodes
            .iter()
            .copied()
            .filter(|&id| matches!(tree.kind(id), Kind::Choice | Kind::Nature))
            .collect()
    } else {
        Vec::new()
    };
    if !trail.is_empty() {
        w(ws("  // the decision trail: 0 not yet decided, 1 the low branch, 2 the high one"));
        for &n in &trail {
            w(format!("  d{n} : [0..2] init 0;  // node {n}, {}", label_of(tree, n)));
        }
        w(ws(""));
    }
    let mark = |n: u32, branch: i32| -> String {
        if trail.contains(&n) {
            format!(" & (d{n}'={branch})")
        } else {
            String::new()
        }
    };

    // (choice)
    if !holder(1).is_empty() {
        w(ws("  // ---- (choice), r(s) = 1 ------------------------------------------------"));
        w(ws("  // A(s) = {low(v), high(v)}: two commands sharing a guard, which is how a"));
        w(ws("  // decision is handed to the scheduler. The two effects differ, so the two"));
        w(ws("  // distributions do not merge in Steps(s)."));
        for &n in holder(1) {
            let g = format!("rank1 & wit1_{n}");
            let node = tree.node(n);
            w(format!(
                "  [choice{n}low]  {g} -> (n{}'=0){};",
                node.low,
                mark(n, 1)
            ));
            w(format!(
                "  [choice{n}high] {g} -> (n{}'=0){};",
                node.high,
                mark(n, 2)
            ));
        }
        w(ws(""));
    }

    // (down)
    if !holder(2).is_empty() {
        w(ws("  // ---- (down), r(s) = 2 --------------------------------------------------"));
        w(ws("  // tau starts the low child of a sequence and both children of a parallel."));
        for &n in holder(2) {
            let g = format!("rank2 & wit2_{n}");
            let node = tree.node(n);
            let upd = if node.kind == Kind::Sequence {
                format!("(n{}'=0)", node.low)
            } else {
                format!("(n{}'=0) & (n{}'=0)", node.low, node.high)
            };
            w(format!("  [down{n}] {g} -> {upd};  // {}", kind_str(node.kind)));
        }
        w(ws(""));
    }

    // (up)
    if !holder(3).is_empty() {
        w(ws("  // ---- (up), r(s) = 3 ----------------------------------------------------"));
        w(ws("  // DEPARTURE: the clause has two cases. They are conditions on the source"));
        w(ws("  // state that exclude one another, so they become two commands with"));
        w(ws("  // disjoint guards, and only a sequence node receives both. The first hands"));
        w(ws("  // a sequence over from its first region to its second; the second closes"));
        w(ws("  // the node above the branches that are done."));
        for &n in holder(3) {
            let g = format!("rank3 & wit3_{n}");
            let node = tree.node(n);
            let (lo, hi, me) = (
                format!("n{}", node.low),
                format!("n{}", node.high),
                format!("n{n}"),
            );
            if node.kind == Kind::Sequence {
                let hand = format!("{lo}=-2 & {hi}=-1");
                w(format!(
                    "  [up{n}hand]  {g} & ({hand}) -> ({lo}'=-1) & ({hi}'=0);"
                ));
                w(format!(
                    "  [up{n}close] {g} & !({hand}) -> ({lo}'=-1) & ({hi}'=-1) & ({me}'=-2);"
                ));
            } else {
                w(format!(
                    "  [up{n}close] {g} -> ({lo}'=-1) & ({hi}'=-1) & ({me}'=-2);  // {}",
                    kind_str(node.kind)
                ));
            }
        }
        w(ws(""));
    }

    // (nature)
    if !holder(4).is_empty() {
        w(ws("  // ---- (nature), r(s) = 4 ------------------------------------------------"));
        w(ws("  // one command with a probabilistic update, the low branch carrying p."));
        for &n in holder(4) {
            let g = format!("rank4 & wit4_{n}");
            let node = tree.node(n);
            w(format!(
                "  [nature{n}] {g} -> p{n} : (n{}'=0){} + (1-p{n}) : (n{}'=0){};",
                node.low,
                mark(n, 1),
                node.high,
                mark(n, 2)
            ));
        }
        w(ws(""));
    }

    // (fast forward)
    w(ws("  // ---- (fast forward), r(s) = 5 ------------------------------------------"));
    w(ws("  // The clause completes every closer and advances every other running node by"));
    w(ws("  // t, tasks and internal nodes alike; the successor is given pointwise, so"));
    w(ws("  // every node is assigned."));
    w(ws("  // DEPARTURE: the effect never mentions the witness, the clause being global."));
    w(ws("  // One command per task is emitted all the same, so that commands and clause"));
    w(ws("  // instances stay in bijection; the guards exclude one another, so at most one"));
    w(ws("  // is enabled, and identical distributions would in any case merge in Steps(s)."));
    let updates = nodes
        .iter()
        .map(|&n| format!("(n{n}'=ff{n})"))
        .collect::<Vec<_>>()
        .join(" & ");
    for &t in &tasks {
        w(format!("  [ff] rank5 & wit5_{t} -> {updates};"));
    }
    w(ws(""));

    // (absorption)
    w(ws("  // ---- (absorption) ------------------------------------------------------"));
    w(ws("  // s = s_F, the final state, where no trigger holds. DEPARTURE: the self loop"));
    w(ws("  // is written out, since PRISM would otherwise report a deadlock and patch it."));
    w(format!("  [absorb] n{0}=-2 -> (n{0}'=-2);", tree.root));
    w(ws(""));
    w(ws("endmodule"));
    w(ws(""));
    w(ws("// the final state of Definition 7: the root is done"));
    w(format!("label \"final\" = n{}=-2;", tree.root));
    w(ws(""));

    // -- rewards -----------------------------------------------------------
    w(ws("// ---------------------------------------------------------------------------"));
    w(ws("// the cost of Definition 3, one vector-valued function, becomes k reward"));
    w(ws("// structures, k being the dimension of the instance. They are TRANSITION"));
    w(ws("// rewards, attached to the fast forward transitions, which are the only ones"));
    w(ws("// that pay: a multi-objective query over an MDP accepts transition rewards"));
    w(ws("// only. Each structure is a single item summing one conditional term per task,"));
    w(ws("// rather than one item per task, because the manual documents the summing of"));
    w(ws("// several matching items for state rewards and does not repeat the statement"));
    w(ws("// for transition rewards. The guard of an item is read in the SOURCE state,"));
    w(ws("// which is where Closers(s) is read as well."));
    w(ws("// ---------------------------------------------------------------------------"));
    for j in 0..dimension {
        w(format!("rewards \"impact{j}\""));
        w(ws("  [ff] true :"));
        for (pos, &t) in tasks.iter().enumerate() {
            let plus = if pos == 0 { "  " } else { "+ " };
            w(format!(
                "      {plus}(closes{t} ? {} : 0.0)   // {}",
                dbl(tree.impact(t)[j]),
                tree.task_names[t as usize]
            ));
        }
        // the semicolon sits on a line of its own: a // comment runs to the
        // end of the line, so a semicolon placed after the last comment
        // would be eaten
        w(ws("  ;"));
        w(ws("endrewards"));
        w(ws(""));
    }
    out.join("\n") + "\n"
}
