//! The full single-step MDP of an instance: every state the transition
//! relation reaches from the initial state, and every move as its own
//! transition, nothing compressed. The search never builds this object,
//! its macro step jumping from one choice state to the next; this module
//! exists for inspection, for drawing, and for checking the semantics
//! against an independent computation, and it is priced accordingly: the
//! state space is exponential in the instance, so `explore` carries a cap
//! and stops with an error rather than exhausting memory.

use std::collections::HashMap;

use crate::state::{Engine, State};
use crate::tree::Tree;

/// One transition of the MDP, from state index to state index.
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub label: Label,
}

pub enum Label {
    /// Rank 1: the decision starting the child carried, an action.
    Action { child: u32 },
    /// Rank 4: the draw starting the child carried, with its probability.
    Draw { child: u32, prob: f64 },
    /// Rank 2: `tau`, the descent at the witness.
    Down { at: u32 },
    /// Rank 3: `tau`, the hand-over or the closing at the witness.
    Up { at: u32 },
    /// Rank 5: the fast forward, the only move that pays: the time it
    /// advances and the cost of the tasks that complete.
    FastForward { t: i32, cost: Vec<f64> },
}

pub struct Mdp {
    pub states: Vec<State>,
    /// The rank and the witness of each state, `None` at the final state.
    pub ranks: Vec<Option<(u8, u32)>>,
    pub edges: Vec<Edge>,
}

/// Walks the whole reachable state space, breadth first, or fails past
/// `max_states`.
pub fn explore(tree: &Tree, max_states: usize) -> Result<Mdp, String> {
    let eng = Engine::new(tree);
    let mut index: HashMap<State, usize> = HashMap::new();
    let mut states: Vec<State> = vec![eng.initial_state()];
    index.insert(states[0].clone(), 0);
    let mut ranks: Vec<Option<(u8, u32)>> = Vec::new();
    let mut edges: Vec<Edge> = Vec::new();

    let mut intern = |ns: State, states: &mut Vec<State>| -> Result<usize, String> {
        if let Some(&j) = index.get(&ns) {
            return Ok(j);
        }
        if states.len() >= max_states {
            return Err(format!("more than {max_states} states; raise --max-states"));
        }
        let j = states.len();
        index.insert(ns.clone(), j);
        states.push(ns);
        Ok(j)
    };

    let mut i = 0;
    while i < states.len() {
        let s = states[i].clone();
        let rw = eng.rank_witness(&s);
        ranks.push(rw);
        match rw {
            None => {} // the final state absorbs; the self loop is implied
            Some((1, v)) => {
                let n = tree.node(v);
                for child in [n.low, n.high] {
                    let mut ns = s.clone();
                    ns[child as usize] = 0;
                    let j = intern(ns, &mut states)?;
                    edges.push(Edge { from: i, to: j, label: Label::Action { child } });
                }
            }
            Some((4, v)) => {
                let n = tree.node(v);
                for (child, prob) in [(n.low, n.prob), (n.high, 1.0 - n.prob)] {
                    let mut ns = s.clone();
                    ns[child as usize] = 0;
                    let j = intern(ns, &mut states)?;
                    edges.push(Edge { from: i, to: j, label: Label::Draw { child, prob } });
                }
            }
            Some((2, v)) => {
                let mut ns = s.clone();
                eng.down(&mut ns, v);
                let j = intern(ns, &mut states)?;
                edges.push(Edge { from: i, to: j, label: Label::Down { at: v } });
            }
            Some((3, v)) => {
                let mut ns = s.clone();
                eng.up(&mut ns, v);
                let j = intern(ns, &mut states)?;
                edges.push(Edge { from: i, to: j, label: Label::Up { at: v } });
            }
            Some((5, _)) => {
                // the step, recomputed here for the label alone
                let mut t = i32::MAX;
                for &u in &tree.tasks {
                    let sv = s[u as usize];
                    if sv >= 0 {
                        let left = tree.node(u).duration as i32 - sv as i32;
                        if left < t {
                            t = left;
                        }
                    }
                }
                let mut ns = s.clone();
                let mut cost = vec![0.0; tree.k];
                eng.fast_forward(&mut ns, &mut cost);
                let j = intern(ns, &mut states)?;
                edges.push(Edge { from: i, to: j, label: Label::FastForward { t, cost } });
            }
            Some((r, _)) => unreachable!("rank {r} is not a clause"),
        }
        i += 1;
    }
    Ok(Mdp { states, ranks, edges })
}

fn fmt_cost(cost: &[f64]) -> String {
    let parts: Vec<String> = cost
        .iter()
        .map(|v| {
            if v.fract() == 0.0 && v.abs() < 1e15 {
                format!("{}", *v as i64)
            } else {
                format!("{v}")
            }
        })
        .collect();
    format!("[{}]", parts.join(", "))
}

/// The dump: the node table of the tree, every state with its rank and
/// witness, and every transition with its label, one line each.
pub fn dump(tree: &Tree, mdp: &Mdp) -> String {
    let mut out = String::new();
    out.push_str(&format!("nodes {}\n", tree.n_nodes));
    out.push_str(&format!("dimensions {}\n", tree.k));
    if !tree.meta.impact_names.is_empty() {
        out.push_str(&format!("impact_names [{}]\n", tree.meta.impact_names.join(", ")));
    }
    for id in 1..=tree.n_nodes {
        let n = tree.node(id);
        match n.kind {
            crate::tree::Kind::Task => out.push_str(&format!(
                "node {id} task {} duration {} impact {}\n",
                tree.task_names[id as usize],
                n.duration,
                fmt_cost(tree.impact(id))
            )),
            crate::tree::Kind::Nature => out.push_str(&format!(
                "node {id} nature {} low {} high {}\n",
                n.prob, n.low, n.high
            )),
            k => out.push_str(&format!(
                "node {id} {} low {} high {}\n",
                match k {
                    crate::tree::Kind::Sequence => "sequence",
                    crate::tree::Kind::Parallel => "parallel",
                    crate::tree::Kind::Choice => "choice",
                    _ => unreachable!(),
                },
                n.low,
                n.high
            )),
        }
    }
    out.push_str(&format!("states {}\n", mdp.states.len()));
    for (i, s) in mdp.states.iter().enumerate() {
        let values: Vec<String> = s[1..].iter().map(|v| v.to_string()).collect();
        match mdp.ranks[i] {
            None => out.push_str(&format!("state {i} final values {}\n", values.join(" "))),
            Some((r, w)) => out.push_str(&format!(
                "state {i} rank {r} witness {w} values {}\n",
                values.join(" ")
            )),
        }
    }
    out.push_str(&format!("edges {}\n", mdp.edges.len()));
    for e in &mdp.edges {
        let line = match &e.label {
            Label::Action { child } => format!("edge {} {} action {child}\n", e.from, e.to),
            Label::Draw { child, prob } => {
                format!("edge {} {} draw {child} {prob}\n", e.from, e.to)
            }
            Label::Down { at } => format!("edge {} {} down {at}\n", e.from, e.to),
            Label::Up { at } => format!("edge {} {} up {at}\n", e.from, e.to),
            Label::FastForward { t, cost } => {
                format!("edge {} {} ff {t} {}\n", e.from, e.to, fmt_cost(cost))
            }
        };
        out.push_str(&line);
    }
    out
}
