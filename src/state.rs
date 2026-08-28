//! States, triggers and the transition relation of Definitions 6 to 9, plus the
//! macro step the search actually walks.
//!
//! A state maps every node to `-1` when it is idle, `-2` when it is done, and to
//! its elapsed progress when it is running. The three phases and the five
//! triggers are transcribed from Definition 8, and the six clauses from
//! Definition 9, in the order of their rank: choice, down, up, nature, fast
//! forward, absorption. The witness of a rank is the holder of least identifier,
//! and since the arena is indexed by the identifier a scan in increasing index
//! is a scan in the order the definition asks for.
//!
//! ## One departure, and why it is sound
//!
//! Definition 9 advances every running node by `t` at a fast forward, "tasks and
//! internal nodes alike". The elapsed progress of an **internal** node is then
//! never read again: the five triggers test an internal node only for its sign,
//! `t` and the closers range over tasks alone, the cost sums tasks alone, and
//! the recursion of Definition 12 reads phases and nothing else. Two states that
//! differ only in the progress of internal nodes therefore have the same
//! successors up to the same difference, the same probabilities, the same costs
//! and the same bounds, so they are bisimilar and quotienting them changes no
//! value.
//!
//! Quotienting is worth doing: it is what lets a running internal node be one
//! value rather than a counter, which shrinks the key of a choice state and, far
//! more to the point, makes two histories that reach the same configuration at
//! different times share the memo entry instead of computing it twice.
//!
//! The quotient is sound by construction here, no rule below reading the value
//! of an internal node other than through its sign. That the rules themselves
//! are Definition 9 is checked from outside, by the `bounds` command, which
//! computes the least and the greatest expected impact of each component over
//! this state graph and compares them with the numbers Storm returns on the
//! PRISM encoding of the same instance, an encoding that carries the progress of
//! every internal node in full.

use crate::tree::{Kind, Tree};

pub const IDLE: i8 = -1;
pub const DONE: i8 = -2;

/// A state, indexed by node identifier; slot `0` is a filler.
pub type State = Vec<i8>;

/// What a state is, when the search stops at it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stop {
    /// `s_F`, the final absorbing state.
    Final,
    /// A state of rank 1: a choice is to be decided, at the witness carried.
    Choice(u32),
}

pub struct Engine<'t> {
    pub tree: &'t Tree,
    /// A macro step that produces more outcomes than this fails rather than
    /// exhausting memory. The number of outcomes is exponential in the nature
    /// nodes that fire between two choice states.
    pub max_outcomes: usize,
}

/// One outcome of a macro step: the decisions it took, the impact it adds, the
/// probability it multiplies, and the choice state it ends in.
#[derive(Clone, Debug)]
pub struct Outcome {
    pub decisions: Vec<u32>,
    pub impact: Vec<f64>,
    pub prob: f64,
    pub state: State,
    pub stop: Stop,
}

#[derive(Debug)]
pub enum StepError {
    /// More outcomes than `max_outcomes`: the macro step out of one choice
    /// state branches too widely to hold.
    TooManyOutcomes(usize),
}

impl<'t> Engine<'t> {
    pub fn new(tree: &'t Tree) -> Engine<'t> {
        Engine {
            tree,
            max_outcomes: 1 << 22,
        }
    }

    /// `s_0`: the root running, everything else idle (Section 4).
    pub fn initial_state(&self) -> State {
        let mut s = vec![IDLE; self.tree.n_nodes as usize + 1];
        s[self.tree.root as usize] = 0;
        s
    }

    #[inline]
    pub fn is_final(&self, s: &[i8]) -> bool {
        s[self.tree.root as usize] == DONE
    }

    /// The rank `r(s)` and the witness `w(s)` of Definition 8, or `None` at
    /// `s_F`, where no trigger holds.
    ///
    /// One pass in increasing identifier keeps, for each rank, the least
    /// identifier that holds it. Scanning in increasing identifier means the
    /// first rank-1 holder found is the witness of rank 1 and no better rank
    /// exists, which is the only early exit available: a smaller rank may still
    /// turn up at a larger identifier, so nothing else may stop the pass.
    pub fn rank_witness(&self, s: &[i8]) -> Option<(u8, u32)> {
        let t = self.tree;
        let mut best: [u32; 6] = [0; 6];
        for id in 1..=t.n_nodes {
            let n = t.node(id);
            let sv = s[id as usize];
            let rank = match n.kind {
                Kind::Task => {
                    if sv >= 0 && sv < n.duration as i8 {
                        5
                    } else {
                        continue;
                    }
                }
                Kind::Parallel => {
                    let (lo, hi) = (s[n.low as usize], s[n.high as usize]);
                    if lo == DONE && hi == DONE {
                        3
                    } else if sv >= 0 && lo == IDLE && hi == IDLE {
                        2
                    } else {
                        continue;
                    }
                }
                Kind::Sequence => {
                    let (lo, hi) = (s[n.low as usize], s[n.high as usize]);
                    if lo == DONE || hi == DONE {
                        3
                    } else if sv >= 0 && lo == IDLE && hi == IDLE {
                        2
                    } else {
                        continue;
                    }
                }
                Kind::Choice => {
                    let (lo, hi) = (s[n.low as usize], s[n.high as usize]);
                    if lo == DONE || hi == DONE {
                        3
                    } else if sv >= 0 && lo == IDLE && hi == IDLE {
                        1
                    } else {
                        continue;
                    }
                }
                Kind::Nature => {
                    let (lo, hi) = (s[n.low as usize], s[n.high as usize]);
                    if lo == DONE || hi == DONE {
                        3
                    } else if sv >= 0 && lo == IDLE && hi == IDLE {
                        4
                    } else {
                        continue;
                    }
                }
            };
            if best[rank] == 0 {
                best[rank] = id;
                if rank == 1 {
                    return Some((1, id));
                }
            }
        }
        for rank in 2..=5u8 {
            if best[rank as usize] != 0 {
                return Some((rank, best[rank as usize]));
            }
        }
        None
    }

    /// The down clause: `tau` starts the low child of a sequence and both
    /// children of a parallel node.
    #[inline]
    pub(crate) fn down(&self, s: &mut [i8], v: u32) {
        let n = self.tree.node(v);
        s[n.low as usize] = 0;
        if n.kind == Kind::Parallel {
            s[n.high as usize] = 0;
        }
    }

    /// The up clause: the hand-over of a sequence, or the closing of a node
    /// above the branches that are done.
    #[inline]
    pub(crate) fn up(&self, s: &mut [i8], v: u32) {
        let n = self.tree.node(v);
        if n.kind == Kind::Sequence
            && s[n.low as usize] == DONE
            && s[n.high as usize] == IDLE
        {
            s[n.low as usize] = IDLE;
            s[n.high as usize] = 0;
        } else {
            s[n.low as usize] = IDLE;
            s[n.high as usize] = IDLE;
            s[v as usize] = DONE;
        }
    }

    /// The fast-forward clause, the only one that pays. `t` is the least time to
    /// a completion over the running tasks, every task at that time completes in
    /// this one step, and the cost is the sum of their impacts.
    #[inline]
    pub(crate) fn fast_forward(&self, s: &mut [i8], impact: &mut [f64]) {
        let tree = self.tree;
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
        debug_assert!(t > 0 && t != i32::MAX, "the fast forward fires with no running task");
        for &u in &tree.tasks {
            let sv = s[u as usize];
            if sv < 0 {
                continue;
            }
            let left = tree.node(u).duration as i32 - sv as i32;
            if left == t {
                s[u as usize] = DONE;
                let i = tree.impact(u);
                for c in 0..tree.k {
                    impact[c] += i[c];
                }
            } else {
                s[u as usize] = (sv as i32 + t) as i8;
            }
        }
        // The progress of a running internal node is not advanced: it is never
        // read, and quotienting it away is what makes a choice state one byte
        // per node. See the note at the head of this file.
    }

    /// Runs the forced moves from `start`, branching at every nature node, until
    /// each branch reaches a choice state or the final state. The decisions of
    /// each branch are appended to `prefix`, which carries the action taken at
    /// the choice state this macro step leaves, so that the first entry of an
    /// outcome is `low(cs)` or `high(cs)` as the board writes it.
    ///
    /// A macro step is one edge of the search and many transitions of the MDP.
    /// Nothing between two choice states is decided by a policy, so nothing
    /// between them has to be held.
    pub fn macro_step(&self, start: State, prefix: Vec<u32>) -> Result<Vec<Outcome>, StepError> {
        let k = self.tree.k;
        let mut out: Vec<Outcome> = Vec::new();
        let mut stack: Vec<(State, Vec<u32>, Vec<f64>, f64)> =
            vec![(start, prefix, vec![0.0; k], 1.0)];

        while let Some((mut s, mut decisions, mut impact, mut prob)) = stack.pop() {
            loop {
                match self.rank_witness(&s) {
                    None => {
                        out.push(Outcome {
                            decisions,
                            impact,
                            prob,
                            state: s,
                            stop: Stop::Final,
                        });
                        break;
                    }
                    Some((1, v)) => {
                        out.push(Outcome {
                            decisions,
                            impact,
                            prob,
                            state: s,
                            stop: Stop::Choice(v),
                        });
                        break;
                    }
                    Some((4, v)) => {
                        if out.len() + stack.len() + 2 > self.max_outcomes {
                            return Err(StepError::TooManyOutcomes(self.max_outcomes));
                        }
                        let n = self.tree.node(v);
                        let mut s_high = s.clone();
                        s_high[n.high as usize] = 0;
                        let mut d_high = decisions.clone();
                        d_high.push(n.high);
                        stack.push((s_high, d_high, impact.clone(), prob * (1.0 - n.prob)));

                        s[n.low as usize] = 0;
                        decisions.push(n.low);
                        prob *= n.prob;
                    }
                    Some((2, v)) => self.down(&mut s, v),
                    Some((3, v)) => self.up(&mut s, v),
                    Some((5, _)) => self.fast_forward(&mut s, &mut impact),
                    Some((r, _)) => unreachable!("rank {r} is not a clause"),
                }
            }
        }
        Ok(out)
    }

    /// The macro step out of the initial state, which no action precedes: the
    /// `InitHistories` of the board.
    pub fn initial_outcomes(&self) -> Result<Vec<Outcome>, StepError> {
        self.macro_step(self.initial_state(), Vec::new())
    }

    /// The macro step out of the choice state `s` under the action that starts
    /// the child `action` of its witness.
    pub fn step_action(&self, s: &[i8], action: u32) -> Result<Vec<Outcome>, StepError> {
        let mut next = s.to_vec();
        next[action as usize] = 0;
        self.macro_step(next, vec![action])
    }
}
