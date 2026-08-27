//! The exact set of vectors a deterministic policy can achieve, as an oracle for
//! the search.
//!
//! This is what Problem 1 quantifies over, computed by brute force: at a choice
//! state the achievable set is the union of what the two actions achieve, and an
//! action achieves the sum, over the outcomes of its macro step, of the
//! probability of the outcome times its impact plus anything achievable from the
//! choice state it leads to. The outcomes are combined as a product and not as a
//! union, which is exactly the freedom a history-dependent policy has and a
//! memoryless one has not: after one draw of nature it may decide one way and
//! after the other draw the other way.
//!
//! Only the Pareto-minimal vectors are kept. The question asked of the set is
//! whether some vector of it is at most `B`, and a vector that another vector of
//! the set dominates can never be the only witness of that.
//!
//! This is exponential and is meant to be. It exists to answer the same question
//! as the search on instances small enough for both, so that the search can be
//! checked against something that shares none of its machinery.

use std::collections::HashMap;

use crate::state::StepError;
use crate::tables::{Branch, ChoiceState, Store};

#[derive(Debug)]
pub enum BruteError {
    Step(StepError),
    /// A Pareto set passed the cap.
    TooWide(usize),
}

impl From<StepError> for BruteError {
    fn from(e: StepError) -> BruteError {
        BruteError::Step(e)
    }
}

/// Every Pareto-minimal value a deterministic policy achieves from the initial
/// state.
pub fn achievable<'a>(store: &Store<'a>, cap: usize) -> Result<Vec<Vec<f64>>, BruteError> {
    let k = store.tree.k;
    let initial = store.initial_histories()?;
    let mut memo: HashMap<usize, std::rc::Rc<Vec<Vec<f64>>>> = HashMap::new();

    // The initial histories are the outcomes of one macro step out of `s_0`, so
    // they combine as a product exactly as the outcomes of an action do.
    let mut acc: Vec<Vec<f64>> = vec![vec![0.0; k]];
    for h in &initial {
        let tail = set_of(store, h.cs, &mut memo, cap)?;
        acc = combine(&acc, &tail, h.prob, &h.impact, k, cap)?;
    }
    Ok(acc)
}

pub fn meets(set: &[Vec<f64>], threshold: &[f64]) -> bool {
    set.iter()
        .any(|v| v.iter().zip(threshold).all(|(x, b)| x <= b))
}

fn set_of<'a>(
    store: &Store<'a>,
    cs: &'a ChoiceState<'a>,
    memo: &mut HashMap<usize, std::rc::Rc<Vec<Vec<f64>>>>,
    cap: usize,
) -> Result<std::rc::Rc<Vec<Vec<f64>>>, BruteError> {
    let key = cs as *const ChoiceState<'_> as usize;
    if let Some(found) = memo.get(&key) {
        return Ok(found.clone());
    }
    let k = store.tree.k;
    if cs.is_final() {
        let out = std::rc::Rc::new(vec![vec![0.0; k]]);
        memo.insert(key, out.clone());
        return Ok(out);
    }
    let mut union: Vec<Vec<f64>> = Vec::new();
    for branch in [Branch::Low, Branch::High] {
        let exts = store.extensions(cs, branch)?;
        let mut acc: Vec<Vec<f64>> = vec![vec![0.0; k]];
        for ext in exts.iter() {
            let tail = set_of(store, ext.cs, memo, cap)?;
            acc = combine(&acc, &tail, ext.prob, &ext.impact, k, cap)?;
        }
        union.extend(acc);
    }
    let out = std::rc::Rc::new(pareto(union, k));
    if out.len() > cap {
        return Err(BruteError::TooWide(cap));
    }
    memo.insert(key, out.clone());
    Ok(out)
}

/// `{ a + p (i + t) : a in acc, t in tail }`, Pareto filtered.
fn combine(
    acc: &[Vec<f64>],
    tail: &[Vec<f64>],
    p: f64,
    i: &[f64],
    k: usize,
    cap: usize,
) -> Result<Vec<Vec<f64>>, BruteError> {
    if acc.len().saturating_mul(tail.len()) > cap.saturating_mul(64) {
        return Err(BruteError::TooWide(cap));
    }
    let mut out = Vec::with_capacity(acc.len() * tail.len());
    for a in acc {
        for t in tail {
            let mut v = Vec::with_capacity(k);
            for c in 0..k {
                v.push(a[c] + p * (i[c] + t[c]));
            }
            out.push(v);
        }
    }
    let out = pareto(out, k);
    if out.len() > cap {
        return Err(BruteError::TooWide(cap));
    }
    Ok(out)
}

/// Keeps the minimal vectors: one that another dominates componentwise can never
/// be the only witness of a threshold.
fn pareto(mut points: Vec<Vec<f64>>, k: usize) -> Vec<Vec<f64>> {
    points.sort_by(|a, b| {
        for c in 0..k {
            match a[c].partial_cmp(&b[c]) {
                Some(std::cmp::Ordering::Equal) => continue,
                Some(o) => return o,
                None => return std::cmp::Ordering::Equal,
            }
        }
        std::cmp::Ordering::Equal
    });
    let mut kept: Vec<Vec<f64>> = Vec::new();
    'outer: for p in points {
        for q in &kept {
            if q.iter().zip(&p).all(|(x, y)| x <= y) {
                continue 'outer;
            }
        }
        kept.retain(|q| !p.iter().zip(q).all(|(x, y)| x <= y));
        kept.push(p);
    }
    kept
}
