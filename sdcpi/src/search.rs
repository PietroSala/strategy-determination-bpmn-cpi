//! The search: one computation tree, several workers.
//!
//! A node of the computation tree is a frontier, a set of histories no one of
//! which is a prefix of another and whose probabilities sum to one. A worker
//! that holds a node tests the two bounds on it, and when neither fires it
//! elects one open history, resolves the choice its state ends at, keeps the low
//! branch and pushes the high one on its stack.
//!
//! Workers hold a stack each and are arranged in a ring: a worker whose stack is
//! empty takes from the **bottom** of the stack of its predecessor, and sleeps
//! when both are empty. The computation ends when a frontier passes the
//! accepting test, which stops every worker at once, or when every worker is
//! asleep, which is the case where no strategy exists.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::tables::{Branch, Expansion, History, Records, Store};
use crate::tree::Tree;

// ---------------------------------------------------------------------------
// configuration
// ---------------------------------------------------------------------------

/// Which of the two tests is read on a frontier that still has open histories.
/// A frontier whose histories are all final is tested by both whatever the
/// setting: there the two sums coincide with the value itself, so one of the two
/// answers, and the search would otherwise never stop.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ablation {
    Both,
    AcceptOnly,
    RejectOnly,
    Neither,
}

impl Ablation {
    fn accept_early(self) -> bool {
        matches!(self, Ablation::Both | Ablation::AcceptOnly)
    }
    fn reject_early(self) -> bool {
        matches!(self, Ablation::Both | Ablation::RejectOnly)
    }
    pub fn parse(s: &str) -> Option<Ablation> {
        match s {
            "both" => Some(Ablation::Both),
            "accept" => Some(Ablation::AcceptOnly),
            "reject" => Some(Ablation::RejectOnly),
            "none" => Some(Ablation::Neither),
            _ => None,
        }
    }
}

/// How the open history to expand is elected. The board leaves this open and
/// marks it for parameterization; what it prescribes for the moment is the
/// weighted draw.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Selection {
    /// Rescale the probabilities of the open histories to one and draw.
    Weighted,
    /// Draw uniformly among them, ignoring the probabilities.
    Uniform,
    /// The open history of least identifier, which is the one created first: no
    /// draw at all, and the only setting under which several workers give a
    /// reproducible run.
    Oldest,
}

impl Selection {
    pub fn parse(s: &str) -> Option<Selection> {
        match s {
            "weighted" => Some(Selection::Weighted),
            "uniform" => Some(Selection::Uniform),
            "oldest" => Some(Selection::Oldest),
            _ => None,
        }
    }
}

/// Where a worker with an empty stack looks for work.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Steal {
    /// From the stack of its predecessor in the ring, which is what the board
    /// prescribes.
    Ring,
    /// From any other stack, scanning from its predecessor backwards. The board
    /// does not ask for this; it is here because the ring propagates work one
    /// hop at a time, and the difference between the two is worth a number
    /// rather than an opinion.
    Any,
}

impl Steal {
    pub fn parse(s: &str) -> Option<Steal> {
        match s {
            "ring" => Some(Steal::Ring),
            "any" => Some(Steal::Any),
            _ => None,
        }
    }
}

pub struct Config {
    pub threshold: Vec<f64>,
    /// Whether to keep the decisions taken, so that the strategy can be
    /// returned. When the caller only wants the answer there is no reason to
    /// build the chain: it costs an allocation and a shared counter per
    /// expansion and is thrown away.
    pub record_strategy: bool,
    pub steal: Steal,
    /// A relative slack on the comparison with the threshold, `0.0` by default.
    ///
    /// The two sums are floating point numbers, and a threshold placed exactly
    /// at an optimum sits exactly on one of them: the value a policy accumulates
    /// along the frontier and the same value accumulated in another order differ
    /// in the last bits, so a comparison that is an equality on paper falls
    /// either way in the machine. A slack of a few units in the last place
    /// settles those cases and is far below anything the instances distinguish.
    pub epsilon: f64,
    pub workers: usize,
    pub ablation: Ablation,
    pub selection: Selection,
    pub seed: u64,
    pub timeout: Option<Duration>,
}

// ---------------------------------------------------------------------------
// the frontier
// ---------------------------------------------------------------------------

/// A frontier: the open histories, and the three running sums.
///
/// The open histories are a flat array of references into the arena, copied at
/// every expansion. A persistent tree was the first shape here, and it was the
/// wrong one: it allocates a node per level per operation and it clones a
/// reference-counted pointer per node, which two profiles running showed to be
/// where sixteen workers spent their time. The frontiers of these processes hold
/// tens of histories, so copying the array outright is a memcpy of a few hundred
/// bytes against dozens of allocations, and it touches no counter any other
/// worker owns.
///
/// `sums` holds three vectors of `k` end to end: what the histories that are
/// already final have paid, and what the open ones contribute to the accepting
/// and to the rejecting sum. The last two are carried forward by adding the
/// terms of an expansion and taking away the term of the history it replaced,
/// and they are **set to zero** when the open array empties rather than left to
/// whatever the additions and subtractions leave behind. That is what makes the
/// two sums the same number on a frontier whose histories are all final, where
/// they are the same number by definition and one of the two tests has to
/// answer.
#[derive(Clone)]
pub struct Frontier<'a> {
    pub open: Box<[&'a History<'a>]>,
    sums: Box<[f64]>,
    /// Every history the frontier holds, the final ones included. They are
    /// summed into the closed part and not kept, so counting them here is the
    /// only way to say how large a partial strategy is: `open` alone is zero
    /// wherever the accepting test is read at a closed frontier, which is every
    /// success once the upper bound is ablated away.
    total: usize,
}

impl<'a> Frontier<'a> {
    /// `E(F) = sum over F of P(h) (cost(h) + U_last(h))`, written into `out`.
    #[inline]
    fn accepting(&self, out: &mut [f64]) {
        let k = out.len();
        for c in 0..k {
            out[c] = self.sums[c] + self.sums[k + c];
        }
    }

    /// `A(F) = sum over F of P(h) cost(h)`, what the frontier has already paid,
    /// written into `out`. Costs are non-negative, so this never decreases as
    /// the frontier is expanded and a component of it above the threshold is a
    /// completion of the search that cannot exist. It is the rejecting test
    /// with `L` taken to be zero, and it is what remains when the lower bound is
    /// ablated away: arithmetic rather than a bound.
    #[inline]
    fn accumulated(&self, out: &mut [f64]) {
        let k = out.len();
        out.copy_from_slice(&self.sums[..k]);
    }

    /// `L(F) = sum over F of P(h) (cost(h) + L_last(h))`, written into `out`.
    #[inline]
    fn rejecting(&self, out: &mut [f64]) {
        let k = out.len();
        for c in 0..k {
            out[c] = self.sums[c] + self.sums[2 * k + c];
        }
    }

    fn from_histories(k: usize, histories: &[&'a History<'a>]) -> Frontier<'a> {
        let mut sums = vec![0.0; 3 * k];
        let mut open = Vec::new();
        for h in histories {
            if h.is_open() {
                for c in 0..k {
                    sums[k + c] += h.e_hat[c];
                    sums[2 * k + c] += h.l_hat[c];
                }
                open.push(*h);
            } else {
                for c in 0..k {
                    sums[c] += h.e_hat[c];
                }
            }
        }
        if open.is_empty() {
            for c in k..3 * k {
                sums[c] = 0.0;
            }
        }
        Frontier {
            total: histories.len(),
            open: open.into_boxed_slice(),
            sums: sums.into_boxed_slice(),
        }
    }

    /// Replaces the elected history by the histories one action leads to.
    fn expand(&self, k: usize, elected: &'a History<'a>, exp: &Expansion<'a>) -> Frontier<'a> {
        let mut open = Vec::with_capacity(self.open.len() + exp.open_children.len());
        for h in self.open.iter() {
            if h.id != elected.id {
                open.push(*h);
            }
        }
        open.extend_from_slice(&exp.open_children);
        let mut sums = self.sums.to_vec();
        for c in 0..k {
            sums[c] += exp.closed_delta[c];
            sums[k + c] += exp.open_e_delta[c] - elected.e_hat[c];
            sums[2 * k + c] += exp.open_l_delta[c] - elected.l_hat[c];
        }
        if open.is_empty() {
            for c in k..3 * k {
                sums[c] = 0.0;
            }
        }
        Frontier {
            total: self.total - 1 + exp.open_children.len() + exp.closed_count,
            open: open.into_boxed_slice(),
            sums: sums.into_boxed_slice(),
        }
    }
}

#[inline]
fn leq(a: &[f64], b: &[f64], epsilon: f64) -> bool {
    if epsilon == 0.0 {
        return a.iter().zip(b).all(|(x, y)| x <= y);
    }
    a.iter()
        .zip(b)
        .all(|(x, y)| *x <= *y + epsilon * y.abs().max(1.0))
}

// ---------------------------------------------------------------------------
// the computation tree
// ---------------------------------------------------------------------------

/// The decisions taken so far, as a chain shared by every node below them.
pub struct SigmaNode {
    parent: Option<Arc<SigmaNode>>,
    dh: Box<[u32]>,
    action: u32,
}

pub struct CtNode<'a> {
    frontier: Frontier<'a>,
    sigma: Option<Arc<SigmaNode>>,
}

/// A deterministic policy, as the synthesis version of the problem asks for it:
/// the branch taken at each history that was decided.
pub struct Strategy {
    pub decisions: Vec<(Vec<u32>, u32)>,
    pub frontier_size: usize,
    /// Every history of the winning frontier, final ones included.
    pub histories: usize,
}

fn resolve(sigma: &Option<Arc<SigmaNode>>) -> Vec<(Vec<u32>, u32)> {
    let mut out = Vec::new();
    let mut cur = sigma.clone();
    while let Some(n) = cur {
        out.push((n.dh.to_vec(), n.action));
        cur = n.parent.clone();
    }
    out.reverse();
    out
}

// ---------------------------------------------------------------------------
// the answer
// ---------------------------------------------------------------------------

pub enum Answer {
    Yes(Strategy),
    No,
    Timeout,
    Failed(String),
}

#[derive(Default)]
pub struct Stats {
    pub expanded: u64,
    pub choice_states: usize,
    pub histories: usize,
    pub macro_steps: usize,
    pub outcomes: usize,
    pub peak_open: u64,
    pub elapsed: Duration,
}

// ---------------------------------------------------------------------------
// the workers
// ---------------------------------------------------------------------------

struct Stack<'a> {
    items: Mutex<VecDeque<CtNode<'a>>>,
    len: AtomicUsize,
}

impl<'a> Stack<'a> {
    fn new() -> Stack<'a> {
        Stack {
            items: Mutex::new(VecDeque::new()),
            len: AtomicUsize::new(0),
        }
    }
    fn push(&self, node: CtNode<'a>) {
        let mut g = self.items.lock().unwrap();
        g.push_back(node);
        self.len.store(g.len(), Ordering::Release);
    }
    /// The owner takes from the top.
    fn pop(&self) -> Option<CtNode<'a>> {
        if self.len.load(Ordering::Acquire) == 0 {
            return None;
        }
        let mut g = self.items.lock().unwrap();
        let out = g.pop_back();
        self.len.store(g.len(), Ordering::Release);
        out
    }
    /// The successor in the ring takes from the bottom.
    fn steal(&self) -> Option<CtNode<'a>> {
        if self.len.load(Ordering::Acquire) == 0 {
            return None;
        }
        let mut g = self.items.lock().unwrap();
        let out = g.pop_front();
        self.len.store(g.len(), Ordering::Release);
        out
    }
    fn is_empty(&self) -> bool {
        self.len.load(Ordering::Acquire) == 0
    }
}

struct Shared<'a> {
    store: Store<'a>,
    cfg: Config,
    stacks: Vec<Stack<'a>>,
    stop: AtomicBool,
    result: Mutex<Option<Answer>>,
    idle: Mutex<usize>,
    wake: Condvar,
    /// How many workers are inside `wait`. A push has to wake somebody only when
    /// this is not zero, and reading it costs one relaxed load: without it every
    /// push takes the one global lock in the program, which on a search of
    /// sixteen million expansions is the whole of the parallelism.
    sleepers: AtomicUsize,
    expanded: AtomicU64,
    peak_open: AtomicU64,
    deadline: Option<Instant>,
}

impl<'a> Shared<'a> {
    fn finish(&self, answer: Answer) {
        let mut slot = self.result.lock().unwrap();
        if slot.is_none() {
            *slot = Some(answer);
        }
        self.stop.store(true, Ordering::SeqCst);
        // Everyone asleep has to learn that it is over.
        let _g = self.idle.lock().unwrap();
        self.wake.notify_all();
    }

    fn push(&self, worker: usize, node: CtNode<'a>) {
        self.stacks[worker].push(node);
        if self.sleepers.load(Ordering::Acquire) != 0 {
            let _g = self.idle.lock().unwrap();
            self.wake.notify_all();
        }
    }

    fn predecessor(&self, worker: usize) -> usize {
        let n = self.stacks.len();
        (worker + n - 1) % n
    }

    fn take(&self, worker: usize) -> Option<CtNode<'a>> {
        if let Some(n) = self.stacks[worker].pop() {
            return Some(n);
        }
        let n = self.stacks.len();
        match self.cfg.steal {
            Steal::Ring => self.stacks[self.predecessor(worker)].steal(),
            Steal::Any => {
                for hop in 1..n {
                    if let Some(node) = self.stacks[(worker + n - hop) % n].steal() {
                        return Some(node);
                    }
                }
                None
            }
        }
    }

    /// Sleeps until there is something to take, and answers whether there is.
    /// A worker counts as idle only while it waits here, so an idle count equal
    /// to the number of workers means no worker can push again and the search is
    /// over with no strategy found.
    fn wait(&self, worker: usize) -> bool {
        let prev = self.predecessor(worker);
        let mut idle = self.idle.lock().unwrap();
        *idle += 1;
        self.sleepers.store(*idle, Ordering::Release);
        if *idle == self.stacks.len() {
            drop(idle);
            self.finish(Answer::No);
            return false;
        }
        loop {
            if self.stop.load(Ordering::Relaxed) {
                *idle -= 1;
                self.sleepers.store(*idle, Ordering::Release);
                return false;
            }
            let anything = match self.cfg.steal {
                Steal::Ring => !self.stacks[worker].is_empty() || !self.stacks[prev].is_empty(),
                Steal::Any => self.stacks.iter().any(|s| !s.is_empty()),
            };
            if anything {
                *idle -= 1;
                self.sleepers.store(*idle, Ordering::Release);
                return true;
            }
            let (guard, timed) = self
                .wake
                .wait_timeout(idle, Duration::from_millis(50))
                .unwrap();
            idle = guard;
            if timed.timed_out() {
                if let Some(d) = self.deadline {
                    if Instant::now() >= d {
                        *idle -= 1;
                        self.sleepers.store(*idle, Ordering::Release);
                        drop(idle);
                        self.finish(Answer::Timeout);
                        return false;
                    }
                }
            }
        }
    }
}

/// A small seeded generator, so that a run of one worker repeats.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed | 1)
    }
    #[inline]
    fn next_u64(&mut self) -> u64 {
        let mut z = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        self.0 = z;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    #[inline]
    fn unit(&mut self) -> f64 {
        // 53 bits, the mantissa of a double, so the draw is uniform on [0,1).
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }
}

fn elect<'a>(
    open: &[&'a History<'a>],
    selection: Selection,
    rng: &mut Rng,
) -> &'a History<'a> {
    match selection {
        Selection::Weighted => {
            let total: f64 = open.iter().map(|h| h.prob).sum();
            let mut r = rng.unit() * total;
            for h in open {
                if r < h.prob {
                    return h;
                }
                r -= h.prob;
            }
            // Floating point can leave a remainder past the last history; the
            // draw then meant the last one.
            open[open.len() - 1]
        }
        Selection::Uniform => open[(rng.next_u64() % open.len() as u64) as usize],
        Selection::Oldest => open.iter().copied().min_by_key(|h| h.id).unwrap(),
    }
}

fn work<'a>(shared: &Shared<'a>, worker: usize) {
    let mut rng = Rng::new(shared.cfg.seed ^ (worker as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    loop {
        if shared.stop.load(Ordering::Relaxed) {
            return;
        }
        let node = match shared.take(worker) {
            Some(n) => n,
            None => {
                if shared.wait(worker) {
                    continue;
                }
                return;
            }
        };
        run(shared, worker, node, &mut rng);
    }
}

/// One worker on one node, and then on the low branch of its expansion, until a
/// test fires. The high branch goes on the stack, which is what makes the search
/// a depth-first one and what gives the other workers something to steal.
///
/// The stack a worker pushes on is its own and is not shared: a lock taken on
/// every expansion is a lock every worker in the ring queues behind, and on a
/// search of sixteen million expansions that lock was three quarters of the
/// running time and made sixteen workers slower than one. What is shared is an
/// overflow, and the worker feeds it only while it is empty, so a thief always
/// has something to take and the owner pays the lock once per theft rather than
/// once per expansion. The item handed over is the **oldest** the worker holds,
/// which in a depth-first search is the largest subtree left to do.
fn run<'a>(shared: &Shared<'a>, worker: usize, first: CtNode<'a>, rng: &mut Rng) {
    let store = &shared.store;
    let cfg = &shared.cfg;
    let k = store.tree.k;
    let mut scratch = vec![0.0; k];
    let mut local: VecDeque<CtNode<'a>> = VecDeque::new();
    let mut node = first;
    let mut expanded_here: u64 = 0;
    let mut peak_here: u64 = 0;
    // The counters are local and are published on the way out: an atomic add per
    // expansion, shared by every worker, is a cache line every core fights over.
    let publish = |shared: &Shared<'a>, e: u64, p: u64| {
        shared.expanded.fetch_add(e, Ordering::Relaxed);
        shared.peak_open.fetch_max(p, Ordering::Relaxed);
    };
    loop {
        if shared.stop.load(Ordering::Relaxed) {
            publish(shared, expanded_here, peak_here);
            return;
        }
        expanded_here += 1;
        if expanded_here % 4096 == 0 {
            if let Some(d) = shared.deadline {
                if Instant::now() >= d {
                    publish(shared, expanded_here, peak_here);
                    shared.finish(Answer::Timeout);
                    return;
                }
            }
        }

        let closed = node.frontier.open.is_empty();
        let open_now = node.frontier.open.len() as u64;
        if open_now > peak_here {
            peak_here = open_now;
        }

        if cfg.ablation.accept_early() || closed {
            node.frontier.accepting(&mut scratch);
            if leq(&scratch, &cfg.threshold, cfg.epsilon) {
                publish(shared, expanded_here, peak_here);
                shared.finish(Answer::Yes(Strategy {
                    decisions: resolve(&node.sigma),
                    frontier_size: open_now as usize,
                    histories: node.frontier.total,
                }));
                return;
            }
        }
        if cfg.ablation.reject_early() || closed {
            node.frontier.rejecting(&mut scratch);
        } else {
            // the lower bound is ablated away, and what is left is not nothing:
            // a frontier that has already paid more than the threshold in some
            // component cannot be completed into a strategy that has not
            node.frontier.accumulated(&mut scratch);
        }
        {
            if !leq(&scratch, &cfg.threshold, cfg.epsilon) {
                match local.pop_back() {
                    Some(next) => {
                        node = next;
                        continue;
                    }
                    None => {
                        publish(shared, expanded_here, peak_here);
                        return;
                    }
                }
            }
        }
        if closed {
            // Both tests were taken, the two sums being the same number here,
            // and the accepting one failed: this branch carries no strategy.
            match local.pop_back() {
                Some(next) => {
                    node = next;
                    continue;
                }
                None => {
                    publish(shared, expanded_here, peak_here);
                    return;
                }
            }
        }

        let elected = elect(&node.frontier.open, cfg.selection, rng);
        let cs = elected.cs;
        let low = match store.children(elected, Branch::Low) {
            Ok(v) => v,
            Err(e) => {
                publish(shared, expanded_here, peak_here);
                shared.finish(Answer::Failed(format!("{e:?}")));
                return;
            }
        };
        let high = match store.children(elected, Branch::High) {
            Ok(v) => v,
            Err(e) => {
                publish(shared, expanded_here, peak_here);
                shared.finish(Answer::Failed(format!("{e:?}")));
                return;
            }
        };

        let (low_sigma, high_sigma) = if cfg.record_strategy {
            (
                Some(Arc::new(SigmaNode {
                    parent: node.sigma.clone(),
                    dh: elected.dh.clone(),
                    action: cs.action(store.tree, Branch::Low),
                })),
                Some(Arc::new(SigmaNode {
                    parent: node.sigma.clone(),
                    dh: elected.dh.clone(),
                    action: cs.action(store.tree, Branch::High),
                })),
            )
        } else {
            (None, None)
        };
        let low_child = CtNode {
            frontier: node.frontier.expand(k, elected, low),
            sigma: low_sigma,
        };
        let high_child = CtNode {
            frontier: node.frontier.expand(k, elected, high),
            sigma: high_sigma,
        };

        local.push_back(high_child);
        while local.len() > 1 && shared.stacks[worker].is_empty() {
            if let Some(handover) = local.pop_front() {
                shared.push(worker, handover);
            }
        }
        node = low_child;
    }
}

/// Runs the search on one instance and one threshold.
pub fn search(tree: &Tree, cfg: Config) -> (Answer, Stats) {
    let records = Records::new();
    search_in(tree, &records, cfg)
}

fn search_in<'a>(tree: &'a Tree, records: &'a Records<'a>, cfg: Config) -> (Answer, Stats) {
    let started = Instant::now();
    let workers = cfg.workers.max(1);
    let deadline = cfg.timeout.map(|d| started + d);
    let store = Store::new(tree, records, (workers * 8).next_power_of_two());

    let initial = match store.initial_histories() {
        Ok(v) => v,
        Err(e) => {
            return (
                Answer::Failed(format!("{e:?}")),
                Stats {
                    elapsed: started.elapsed(),
                    ..Default::default()
                },
            )
        }
    };
    let root = CtNode {
        frontier: Frontier::from_histories(tree.k, &initial),
        sigma: None,
    };

    let shared = Shared {
        store,
        cfg: Config { workers, ..cfg },
        stacks: (0..workers).map(|_| Stack::new()).collect(),
        stop: AtomicBool::new(false),
        result: Mutex::new(None),
        idle: Mutex::new(0),
        wake: Condvar::new(),
        sleepers: AtomicUsize::new(0),
        expanded: AtomicU64::new(0),
        peak_open: AtomicU64::new(0),
        deadline,
    };

    // Worker 0 starts on the root, as the board has it; the others find their
    // stacks empty and sleep until something reaches them around the ring.
    shared.stacks[0].push(root);

    std::thread::scope(|scope| {
        for w in 0..workers {
            let shared = &shared;
            scope.spawn(move || work(shared, w));
        }
    });

    let answer = shared
        .result
        .lock()
        .unwrap()
        .take()
        .unwrap_or(Answer::No);
    let stats = Stats {
        expanded: shared.expanded.load(Ordering::Relaxed),
        choice_states: shared.store.cs_made.load(Ordering::Relaxed),
        histories: shared.store.hist_made.load(Ordering::Relaxed),
        macro_steps: shared.store.macro_steps.load(Ordering::Relaxed),
        outcomes: shared.store.outcomes.load(Ordering::Relaxed),
        peak_open: shared.peak_open.load(Ordering::Relaxed),
        elapsed: started.elapsed(),
    };
    (answer, stats)
}
