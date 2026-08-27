//! The least and the greatest expected impact of each component, computed
//! exactly over the graph of the choice states.
//!
//! This is not the search: it optimises one component at a time, which is a
//! single-objective question and has nothing to say about a threshold on a
//! vector. It is here for two reasons. It is the scale on which a threshold is
//! placed, a bound above every maximum being a yes for any strategy and one
//! below some minimum a no for all of them. And it is how the transition
//! relation of this crate is checked from outside: Storm answers the same
//! question on the PRISM encoding of the same instance, an encoding that carries
//! the progress of every internal node in full, so agreement digit for digit is
//! evidence that the quotient of `state.rs` changes no value.
//!
//! The recursion terminates because the graph is acyclic: every macro step
//! completes at least one task or closes at least one node, and Corollary 2
//! bounds a run by `4|V|` transitions.

use std::collections::HashMap;

use crate::state::StepError;
use crate::tables::{Branch, ChoiceState, Store};

pub struct Optima {
    pub min: Vec<f64>,
    pub max: Vec<f64>,
}

/// Why an exact pass gave up, when it did.
#[derive(Debug)]
pub enum ExactError {
    Step(StepError),
    /// The graph of the choice states passed the cap. The whole graph is what
    /// the search exists not to build, so a cap here is not a limitation of the
    /// search, only of this check.
    TooManyStates(usize),
}

impl From<StepError> for ExactError {
    fn from(e: StepError) -> ExactError {
        ExactError::Step(e)
    }
}

/// How far the recursion of Definition 12 sits from the exact optimum, over
/// every choice state the process can reach: the largest difference between
/// `U_s` and the greatest expected impact still to be paid from `s`, and between
/// `L_s` and the least.
pub fn tightness<'a>(store: &Store<'a>, max_states: usize) -> Result<(f64, f64, usize), ExactError> {
    let k = store.tree.k;
    let initial = store.initial_histories()?;
    let mut memo: HashMap<usize, (Vec<f64>, Vec<f64>)> = HashMap::new();
    let mut seen: Vec<&ChoiceState<'_>> = Vec::new();
    for h in &initial {
        value(store, h.cs, &mut memo, max_states)?;
        collect(store, h.cs, &mut seen, &mut std::collections::HashSet::new())?;
    }
    let mut du: f64 = 0.0;
    let mut dl: f64 = 0.0;
    for cs in &seen {
        let key = *cs as *const ChoiceState<'_> as usize;
        if let Some((lo, hi)) = memo.get(&key) {
            for c in 0..k {
                du = du.max((cs.upper[c] - hi[c]).abs());
                dl = dl.max((cs.lower[c] - lo[c]).abs());
            }
        }
    }
    Ok((du, dl, seen.len()))
}

fn collect<'a>(
    store: &Store<'a>,
    cs: &'a ChoiceState<'a>,
    out: &mut Vec<&'a ChoiceState<'a>>,
    seen: &mut std::collections::HashSet<usize>,
) -> Result<(), ExactError> {
    let key = cs as *const ChoiceState<'_> as usize;
    if !seen.insert(key) {
        return Ok(());
    }
    out.push(cs);
    if cs.is_final() {
        return Ok(());
    }
    for branch in [Branch::Low, Branch::High] {
        let exts = store.extensions(cs, branch)?;
        for ext in exts.iter() {
            collect(store, ext.cs, out, seen)?;
        }
    }
    Ok(())
}

pub fn optima<'a>(store: &Store<'a>, max_states: usize) -> Result<Optima, ExactError> {
    let k = store.tree.k;
    let initial = store.initial_histories()?;
    let mut memo: HashMap<usize, (Vec<f64>, Vec<f64>)> = HashMap::new();

    let mut min = vec![0.0; k];
    let mut max = vec![0.0; k];
    for h in &initial {
        let (lo, hi) = value(store, h.cs, &mut memo, max_states)?;
        for c in 0..k {
            min[c] += h.prob * (h.impact[c] + lo[c]);
            max[c] += h.prob * (h.impact[c] + hi[c]);
        }
    }
    Ok(Optima { min, max })
}

/// The least and the greatest expected impact still to be paid from a choice
/// state, each component optimised on its own, as a per-component question is.
fn value<'a>(
    store: &Store<'a>,
    cs: &'a ChoiceState<'a>,
    memo: &mut HashMap<usize, (Vec<f64>, Vec<f64>)>,
    max_states: usize,
) -> Result<(Vec<f64>, Vec<f64>), ExactError> {
    let key = cs as *const ChoiceState<'_> as usize;
    if let Some(found) = memo.get(&key) {
        return Ok(found.clone());
    }
    if store.cs_made.load(std::sync::atomic::Ordering::Relaxed) > max_states {
        return Err(ExactError::TooManyStates(max_states));
    }
    let k = store.tree.k;
    if cs.is_final() {
        let zero = vec![0.0; k];
        let out = (zero.clone(), zero);
        memo.insert(key, out.clone());
        return Ok(out);
    }

    let mut best_min: Option<Vec<f64>> = None;
    let mut best_max: Option<Vec<f64>> = None;
    for branch in [Branch::Low, Branch::High] {
        let exts = store.extensions(cs, branch)?;
        let mut lo = vec![0.0; k];
        let mut hi = vec![0.0; k];
        for ext in exts.iter() {
            let (cl, ch) = value(store, ext.cs, memo, max_states)?;
            for c in 0..k {
                lo[c] += ext.prob * (ext.impact[c] + cl[c]);
                hi[c] += ext.prob * (ext.impact[c] + ch[c]);
            }
        }
        best_min = Some(match best_min {
            None => lo,
            Some(prev) => prev.iter().zip(&lo).map(|(a, b)| a.min(*b)).collect(),
        });
        best_max = Some(match best_max {
            None => hi,
            Some(prev) => prev.iter().zip(&hi).map(|(a, b)| a.max(*b)).collect(),
        });
    }
    let out = (best_min.unwrap(), best_max.unwrap());
    memo.insert(key, out.clone());
    Ok(out)
}
