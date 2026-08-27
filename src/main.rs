//! `sdcpi`: on-the-fly strategy synthesis for BPMN+CPI processes.
//!
//!     sdcpi info    <instance>
//!     sdcpi optima  <instance>
//!     sdcpi verify  <instance> [--bounds-dir DIR]
//!     sdcpi search  <instance> (--threshold a,b,... | --alpha X) [options]
//!
//! An instance is either a path to a YAML file or a key of the grid,
//! `<nested>-<independent>-<process_number>-<dimensions>-<mode>`, resolved under
//! `--root` (`bpmn-cpi-benchmarks` beside the executable tree by default).

mod achievable;
mod arena;
mod bound;
mod exact;
mod search;
mod state;
mod tables;
mod tree;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use search::{Ablation, Answer, Config, Selection, Steal};
use tables::Store;
use tree::Tree;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("{}", USAGE);
        std::process::exit(2);
    }
    let code = match args[0].as_str() {
        "info" => cmd_info(&args[1..]),
        "optima" => cmd_optima(&args[1..]),
        "verify" => cmd_verify(&args[1..]),
        "search" => cmd_search(&args[1..]),
        "check" => cmd_check(&args[1..]),
        "bound" => cmd_bound(&args[1..]),
        "tight" => cmd_tight(&args[1..]),
        "sweep" => cmd_sweep(&args[1..]),
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            0
        }
        other => {
            eprintln!("unknown command {other:?}\n{USAGE}");
            2
        }
    };
    std::process::exit(code);
}

const USAGE: &str = "\
sdcpi  on-the-fly strategy synthesis for BPMN+CPI processes

  sdcpi info    <instance>
  sdcpi optima  <instance>
  sdcpi verify  <instance> [--bounds-dir DIR]
  sdcpi search  <instance> (--threshold a,b,... | --alpha X) [options]
  sdcpi bound   <instance>
  sdcpi sweep   <listfile> [--threads N]
  sdcpi check   <instance> [--alphas a,b,...] [--ablations ...] [--cap N]

options for search
  --bound-file F        the bound B from a yaml holding `B: [a, b, ...]`
  --threshold a,b,...   the bound B, one value per component
  --alpha X             B = min + X (max - min), the optima computed first and
                        reported apart from the time of the search itself
  --workers N           number of workers (default 1)
  --ablation MODE       both | accept | reject | none      (default both)
  --selection MODE      weighted | uniform | oldest        (default weighted)
  --seed S              seed of the draw                   (default 20260816)
  --timeout SECS        give up after this many seconds
  --epsilon E           relative slack on the comparison with B (default 0)
  --steal MODE          ring | any                        (default ring)
  --from-bounds 1       place B from the Storm bounds on disk instead of
                        computing the optima here
  --print-size 1        on a positive answer print how large the partial
                        strategy is: the histories of the winning frontier, how
                        many of them are still open, and how many decisions it
                        takes. Neither the strategy nor its decisions are
                        printed
  --print-strategy 1    print the strategy at the end. Without it the decisions
                        are never recorded, which the search is faster for, and
                        only the outcome is reported
  --root DIR            where the grid sits, for a key rather than a path

options for optima and verify
  --max-states N        give up past this many choice states (default 2000000)
";

// ---------------------------------------------------------------------------
// arguments
// ---------------------------------------------------------------------------

struct Args {
    positional: Vec<String>,
    flags: Vec<(String, String)>,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut out = Args {
        positional: Vec::new(),
        flags: Vec::new(),
    };
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if let Some(name) = a.strip_prefix("--") {
            if let Some((n, v)) = name.split_once('=') {
                out.flags.push((n.to_string(), v.to_string()));
                i += 1;
            } else {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| format!("--{name} wants a value"))?;
                out.flags.push((name.to_string(), v.clone()));
                i += 2;
            }
        } else {
            out.positional.push(a.clone());
            i += 1;
        }
    }
    Ok(out)
}

impl Args {
    fn flag(&self, name: &str) -> Option<&str> {
        self.flags
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

fn resolve(spec: &str, root: Option<&str>) -> Result<PathBuf, String> {
    let as_path = Path::new(spec);
    if spec.ends_with(".yaml") || as_path.exists() {
        return Ok(as_path.to_path_buf());
    }
    let parts: Vec<&str> = spec.splitn(5, '-').collect();
    if parts.len() != 5 {
        return Err(format!(
            "{spec:?} is neither a path nor a key of the form nested-independent-number-dimensions-mode"
        ));
    }
    let base = match root {
        Some(r) => PathBuf::from(r),
        None => PathBuf::from("bpmn-cpi-benchmarks"),
    };
    let path = base
        .join(format!("{}-nested", parts[0]))
        .join(format!("{}-independent", parts[1]))
        .join(format!("{}-process_number", parts[2]))
        .join(format!("{}-{}.yaml", parts[3], parts[4]));
    if !path.exists() {
        return Err(format!("{} does not exist", path.display()));
    }
    Ok(path)
}

fn load(args: &Args) -> Result<(PathBuf, Tree), String> {
    let spec = args
        .positional
        .first()
        .ok_or_else(|| "no instance given".to_string())?;
    let path = resolve(spec, args.flag("root"))?;
    let tree = Tree::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((path, tree))
}

/// The threshold from a YAML file holding one key,
///
/// ```yaml
/// B: [0.5, 1.25, 3.0]
/// ```
///
/// one value per component. Other keys are ignored, so a file that also records
/// where the numbers came from stays readable here.
fn bound_file(path: &str) -> Result<Vec<f64>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('#') {
            continue;
        }
        let rest = match t.strip_prefix("B:") {
            Some(r) => r.trim(),
            None => continue,
        };
        let inner = rest
            .strip_prefix('[')
            .and_then(|v| v.strip_suffix(']'))
            .ok_or_else(|| format!("{path}: B must be a list on one line, `B: [a, b, ...]`"))?;
        return vector(inner).map_err(|e| format!("{path}: {e}"));
    }
    Err(format!("{path}: no `B:` key in it"))
}

fn vector(text: &str) -> Result<Vec<f64>, String> {
    text.split(',')
        .map(|p| {
            p.trim()
                .parse::<f64>()
                .map_err(|_| format!("{p:?} is not a number"))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// commands
// ---------------------------------------------------------------------------

fn cmd_info(args: &[String]) -> i32 {
    let args = match parse_args(args) {
        Ok(a) => a,
        Err(e) => return fail(&e),
    };
    let (path, tree) = match load(&args) {
        Ok(v) => v,
        Err(e) => return fail(&e),
    };
    let m = &tree.meta;
    println!("instance      {}", m.key);
    println!("path          {}", path.display());
    println!("grid          nested {} independent {} number {}", m.nested, m.independent, m.process_number);
    println!("dimensions    {}", tree.k);
    println!("mode          {}", m.mode);
    println!("tasks         {}", tree.n_tasks);
    println!("nodes         {}", tree.n_nodes);
    println!("xor nodes     {}", tree.xors.len());
    let (choices, natures) = tree.xors.iter().fold((0, 0), |(c, n), &id| {
        if tree.kind(id) == tree::Kind::Choice {
            (c + 1, n)
        } else {
            (c, n + 1)
        }
    });
    println!("  choice      {choices}");
    println!("  nature      {natures}");
    println!("max duration  {}", m.max_duration);
    let total = tree.total_impact();
    println!("total impact  {}", fmt_vec(&total));
    0
}

fn cmd_optima(args: &[String]) -> i32 {
    let args = match parse_args(args) {
        Ok(a) => a,
        Err(e) => return fail(&e),
    };
    let (_, tree) = match load(&args) {
        Ok(v) => v,
        Err(e) => return fail(&e),
    };
    let records = tables::Records::new();
    let store = Store::new(&tree, &records, 1);
    let max_states = args
        .flag("max-states")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2_000_000usize);
    let started = Instant::now();
    match exact::optima(&store, max_states) {
        Ok(o) => {
            let secs = started.elapsed().as_secs_f64();
            println!("instance      {}", tree.meta.key);
            println!("choice states {}", store.cs_made.load(std::sync::atomic::Ordering::Relaxed));
            println!("seconds       {secs:.4}");
            for c in 0..tree.k {
                // shortest form that reads back as the same double. Ten
                // decimal places would keep four significant digits of a
                // component worth 2e-7, and a reader that pads such a number to
                // make it a bound no policy can fail would be padding under its
                // own rounding
                println!("component {c}  min {:e}  max {:e}", o.min[c], o.max[c]);
            }
            0
        }
        Err(e) => fail(&format!("{e:?}")),
    }
}

fn cmd_verify(args: &[String]) -> i32 {
    let args = match parse_args(args) {
        Ok(a) => a,
        Err(e) => return fail(&e),
    };
    let (path, tree) = match load(&args) {
        Ok(v) => v,
        Err(e) => return fail(&e),
    };
    let records = tables::Records::new();
    let store = Store::new(&tree, &records, 1);
    let max_states = args
        .flag("max-states")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2_000_000usize);
    let ours = match exact::optima(&store, max_states) {
        Ok(o) => o,
        Err(e) => return fail(&format!("{e:?}")),
    };
    let dir = args.flag("bounds-dir").unwrap_or("benchmarks-bounds");
    let reference = match reference_path(&path, dir) {
        Some(p) => p,
        None => return fail("cannot place the instance inside the grid"),
    };
    let text = match std::fs::read_to_string(&reference) {
        Ok(t) => t,
        Err(e) => return fail(&format!("{}: {e}", reference.display())),
    };
    let theirs = match parse_reference(&text) {
        Some(v) => v,
        None => return fail(&format!("{}: no bounds in it", reference.display())),
    };
    if theirs.len() != tree.k {
        return fail(&format!(
            "{} carries {} components against {} declared",
            reference.display(),
            theirs.len(),
            tree.k
        ));
    }
    let mut worst: f64 = 0.0;
    for c in 0..tree.k {
        let dmin = (ours.min[c] - theirs[c].0).abs();
        let dmax = (ours.max[c] - theirs[c].1).abs();
        worst = worst.max(dmin).max(dmax);
        println!(
            "component {c}  min {:.10} / {:.10}  max {:.10} / {:.10}",
            ours.min[c], theirs[c].0, ours.max[c], theirs[c].1
        );
    }
    println!("largest difference {worst:.3e}");
    if worst <= 1e-6 {
        println!("AGREE");
        0
    } else {
        println!("DISAGREE");
        1
    }
}

fn cmd_search(args: &[String]) -> i32 {
    let args = match parse_args(args) {
        Ok(a) => a,
        Err(e) => return fail(&e),
    };
    let (path, tree) = match load(&args) {
        Ok(v) => v,
        Err(e) => return fail(&e),
    };

    let mut optima_secs = 0.0;
    let from_file = match args.flag("bound-file") {
        Some(p) => match bound_file(p) {
            Ok(v) => Some(v),
            Err(e) => return fail(&e),
        },
        None => None,
    };
    let threshold = match (
        from_file.as_ref().map(|v| v.as_slice()),
        args.flag("threshold"),
        args.flag("alpha"),
    ) {
        (Some(v), _, _) if v.len() == tree.k => v.to_vec(),
        (Some(v), _, _) => {
            return fail(&format!(
                "the bound file carries {} components against {} declared",
                v.len(),
                tree.k
            ))
        }
        (None, Some(t), _) => match vector(t) {
            Ok(v) if v.len() == tree.k => v,
            Ok(v) => {
                return fail(&format!(
                    "the threshold has {} components against {} declared",
                    v.len(),
                    tree.k
                ))
            }
            Err(e) => return fail(&e),
        },
        (None, None, Some(a)) => {
            let alpha: f64 = match a.parse() {
                Ok(v) => v,
                Err(_) => return fail("--alpha wants a number"),
            };
            // The optima place the threshold, and computing them exactly walks
            // the whole graph of the choice states, which is what the search
            // exists not to do. On an instance where Storm has already answered
            // that question, `--from-bounds` reads its answer instead, so the
            // grid can be run at sizes the exact pass would not reach.
            if args.flag("from-bounds").is_some() {
                let dir = args.flag("bounds-dir").unwrap_or("benchmarks-bounds");
                let reference = match reference_path(&path, dir) {
                    Some(p) => p,
                    None => return fail("cannot place the instance inside the grid"),
                };
                let text = match std::fs::read_to_string(&reference) {
                    Ok(t) => t,
                    Err(e) => return fail(&format!("{}: {e}", reference.display())),
                };
                let theirs = match parse_reference(&text) {
                    Some(v) if v.len() == tree.k => v,
                    Some(v) => {
                        return fail(&format!(
                            "{} carries {} components against {} declared",
                            reference.display(),
                            v.len(),
                            tree.k
                        ))
                    }
                    None => return fail(&format!("{}: no bounds in it", reference.display())),
                };
                theirs
                    .iter()
                    .map(|(lo, hi)| lo + alpha * (hi - lo))
                    .collect()
            } else {
                let records = tables::Records::new();
                let store = Store::new(&tree, &records, 1);
                let started = Instant::now();
                let o = match exact::optima(&store, 2_000_000usize) {
                    Ok(o) => o,
                    Err(e) => return fail(&format!("{e:?}")),
                };
                optima_secs = started.elapsed().as_secs_f64();
                (0..tree.k)
                    .map(|c| o.min[c] + alpha * (o.max[c] - o.min[c]))
                    .collect()
            }
        }
        (None, None, None) => {
            return fail("give --bound-file, --threshold or --alpha")
        }
    };

    let print_strategy = args.flag("print-strategy").is_some();
    let print_size = args.flag("print-size").is_some();
    let cfg = Config {
        threshold: threshold.clone(),
        // recording costs nothing measurable, 1.7367 s against 1.7363 s on
        // an instance that searches for over a second, and without it the size
        // of the partial strategy cannot be told
        record_strategy: print_strategy || print_size,
        steal: match args.flag("steal").map(Steal::parse) {
            None => Steal::Ring,
            Some(Some(v)) => v,
            Some(None) => return fail("--steal wants ring or any"),
        },
        epsilon: args
            .flag("epsilon")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0),
        workers: args
            .flag("workers")
            .and_then(|v| v.parse().ok())
            .unwrap_or(1),
        ablation: match args.flag("ablation").map(Ablation::parse) {
            None => Ablation::Both,
            Some(Some(a)) => a,
            Some(None) => return fail("--ablation wants both, accept, reject or none"),
        },
        selection: match args.flag("selection").map(Selection::parse) {
            None => Selection::Weighted,
            Some(Some(s)) => s,
            Some(None) => return fail("--selection wants weighted, uniform or oldest"),
        },
        seed: args
            .flag("seed")
            .and_then(|v| v.parse().ok())
            .unwrap_or(20260816),
        timeout: args
            .flag("timeout")
            .and_then(|v| v.parse::<f64>().ok())
            .map(Duration::from_secs_f64),
    };

    let (answer, stats) = search::search(&tree, cfg);

    println!("instance       {}", tree.meta.key);
    println!("threshold      {}", fmt_vec(&threshold));
    if optima_secs > 0.0 {
        println!("optima seconds {optima_secs:.4}");
    }
    println!(
        "answer         {}",
        match &answer {
            Answer::Yes(_) => "yes",
            Answer::No => "no",
            Answer::Timeout => "timeout",
            Answer::Failed(_) => "failed",
        }
    );
    if let Answer::Failed(e) = &answer {
        println!("reason         {e}");
    }
    println!("seconds        {:.6}", stats.elapsed.as_secs_f64());
    println!("expanded       {}", stats.expanded);
    println!("choice states  {}", stats.choice_states);
    println!("histories      {}", stats.histories);
    println!("macro steps    {}", stats.macro_steps);
    println!("step outcomes  {}", stats.outcomes);
    println!("peak open      {}", stats.peak_open);
    // The outcome on its own line, so that a caller reads one number and not
    // prose. A run that timed out or failed has no answer, and saying 0 there
    // would be saying that no strategy exists, which is not what happened.
    println!(
        "outcome        {}",
        match &answer {
            Answer::Yes(_) => "1 strategy found",
            Answer::No => "0 strategy not found",
            Answer::Timeout => "- timed out, no answer",
            Answer::Failed(_) => "- failed, no answer",
        }
    );
    if let Answer::Yes(s) = &answer {
        if print_size || print_strategy {
            // How large the partial strategy is, which is the third thing a
            // caller may want after the outcome and the strategy itself. The
            // histories of the winning frontier are the ones the strategy has
            // to prescribe for, the final ones included; the open ones are the
            // ones it leaves to a bound rather than to a decision, and there
            // are none of those once the accepting test is ablated away.
            println!("histories won  {}", s.histories);
            println!("open won       {}", s.frontier_size);
            println!("decisions won  {}", s.decisions.len());
        }
        if print_strategy {
            println!("decisions      {}", s.decisions.len());
            for (dh, action) in &s.decisions {
                println!("  after {dh:?} take {action}");
            }
        }
    }
    match answer {
        Answer::Failed(_) => 1,
        _ => 0,
    }
}

/// The two bounds at the initial state, and the two sums on the frontier the
/// root of the computation tree carries, beside the exact optima. Everything the
/// accept and the reject test compare with `B`, printed rather than trusted.
fn cmd_bound(args: &[String]) -> i32 {
    let args = match parse_args(args) {
        Ok(a) => a,
        Err(e) => return fail(&e),
    };
    let (_, tree) = match load(&args) {
        Ok(v) => v,
        Err(e) => return fail(&e),
    };
    let records = tables::Records::new();
    let store = Store::new(&tree, &records, 1);
    let engine_state = store.engine.initial_state();
    let b = bound::bounds(&tree, &engine_state);
    println!("instance   {}", tree.meta.key);
    println!("U at s0    {}", fmt_vec(&b.upper));
    println!("L at s0    {}", fmt_vec(&b.lower));
    let initial = match store.initial_histories() {
        Ok(v) => v,
        Err(e) => return fail(&format!("{e:?}")),
    };
    let k = tree.k;
    let (mut e, mut l) = (vec![0.0; k], vec![0.0; k]);
    for h in &initial {
        for c in 0..k {
            e[c] += h.e_hat[c];
            l[c] += h.l_hat[c];
        }
    }
    println!("root E     {}", fmt_vec(&e));
    println!("root L     {}", fmt_vec(&l));
    println!("root size  {} histories", initial.len());
    if args.flag("no-exact").is_none() {
        match exact::optima(&store, 2_000_000usize) {
            Ok(o) => {
                println!("exact min  {}", fmt_vec(&o.min));
                println!("exact max  {}", fmt_vec(&o.max));
            }
            Err(e) => println!("exact      unavailable: {e:?}"),
        }
    }
    0
}

/// How far the two bounds sit from the exact optima, at every reachable choice
/// state and not only at the initial one.
fn cmd_tight(args: &[String]) -> i32 {
    let args = match parse_args(args) {
        Ok(a) => a,
        Err(e) => return fail(&e),
    };
    let (_, tree) = match load(&args) {
        Ok(v) => v,
        Err(e) => return fail(&e),
    };
    let records = tables::Records::new();
    let store = Store::new(&tree, &records, 1);
    let cap: usize = args.flag("max-states").and_then(|v| v.parse().ok()).unwrap_or(400_000);
    match exact::tightness(&store, cap) {
        Ok((du, dl, n)) => {
            println!("instance      {}", tree.meta.key);
            println!("choice states {n}");
            println!("max |U_s - max| {du:.3e}");
            println!("max |L_s - min| {dl:.3e}");
            println!("{}", if du <= 1e-9 && dl <= 1e-9 { "EXACT" } else { "LOOSE" });
            0
        }
        Err(e) => {
            println!("SKIP {e:?}");
            3
        }
    }
}

/// Checks the search against the exact set of achievable vectors, on the same
/// instance and a series of thresholds. The two share the parser, the semantics
/// and the tables, and share nothing of how the question is answered: one
/// enumerates every deterministic policy up to Pareto dominance, the other
/// prunes with the two bounds and stops at the first frontier that passes.
fn cmd_check(args: &[String]) -> i32 {
    let args = match parse_args(args) {
        Ok(a) => a,
        Err(e) => return fail(&e),
    };
    let (_, tree) = match load(&args) {
        Ok(v) => v,
        Err(e) => return fail(&e),
    };
    let cap: usize = args.flag("cap").and_then(|v| v.parse().ok()).unwrap_or(20_000);
    let alphas = match args.flag("alphas") {
        Some(t) => match vector(t) {
            Ok(v) => v,
            Err(e) => return fail(&e),
        },
        None => vec![-0.05, 0.0, 0.1, 0.25, 0.4, 0.5, 0.6, 0.75, 0.9, 1.0, 1.05],
    };

    let records = tables::Records::new();
    let store = Store::new(&tree, &records, 1);
    let o = match exact::optima(&store, 2_000_000usize) {
        Ok(o) => o,
        Err(e) => return fail(&format!("{e:?}")),
    };
    let set = match achievable::achievable(&store, cap) {
        Ok(s) => s,
        Err(e) => {
            println!("instance {} SKIP {e:?}", tree.meta.key);
            return 3;
        }
    };
    println!("instance      {}", tree.meta.key);
    println!("pareto points {}", set.len());

    let workers = args.flag("workers").and_then(|v| v.parse().ok()).unwrap_or(1);
    let selection = match args.flag("selection").map(Selection::parse) {
        None => Selection::Weighted,
        Some(Some(s)) => s,
        Some(None) => return fail("--selection wants weighted, uniform or oldest"),
    };
    let search_timeout = args
        .flag("search-timeout")
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(30.0);
    let ablations: Vec<Ablation> = match args.flag("ablations") {
        Some(t) => {
            let mut v = Vec::new();
            for piece in t.split(',') {
                match Ablation::parse(piece.trim()) {
                    Some(a) => v.push(a),
                    None => return fail("--ablations wants both, accept, reject or none"),
                }
            }
            v
        }
        None => vec![
            Ablation::Both,
            Ablation::AcceptOnly,
            Ablation::RejectOnly,
            Ablation::Neither,
        ],
    };
    let epsilon: f64 = args.flag("epsilon").and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let mut bad = 0;
    for (i, alpha) in alphas.iter().enumerate() {
        let b: Vec<f64> = (0..tree.k)
            .map(|c| o.min[c] + alpha * (o.max[c] - o.min[c]))
            .collect();
        let truth = achievable::meets(&set, &b);
        for &ablation in &ablations {
            let cfg = Config {
                threshold: b.clone(),
                record_strategy: false,
                steal: Steal::Ring,
                epsilon,
                workers,
                ablation,
                selection,
                seed: 20260816 + i as u64,
                timeout: Some(Duration::from_secs_f64(search_timeout)),
            };
            let (answer, _) = search::search(&tree, cfg);
            let got = match answer {
                Answer::Yes(_) => Some(true),
                Answer::No => Some(false),
                _ => None,
            };
            let verdict = match got {
                Some(g) if g == truth => "ok",
                Some(_) => {
                    bad += 1;
                    "MISMATCH"
                }
                None => "inconclusive",
            };
            println!(
                "alpha {alpha:+.2}  truth {truth:<5}  ablation {ablation:?}  {verdict}"
            );
            if verdict == "MISMATCH" {
                println!("    B      {}", fmt_exact(&b));
                for p in set.iter().take(4) {
                    let d: Vec<f64> = p.iter().zip(&b).map(|(x, y)| x - y).collect();
                    println!("    point  {}", fmt_exact(p));
                    println!("    p - B  {}", fmt_exact(&d));
                }
            }
        }
    }
    if bad == 0 {
        println!("ALL AGREE");
        0
    } else {
        println!("{bad} MISMATCHES");
        1
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn fail(message: &str) -> i32 {
    eprintln!("sdcpi: {message}");
    2
}

fn fmt_exact(v: &[f64]) -> String {
    let parts: Vec<String> = v.iter().map(|x| format!("{x:.17e}")).collect();
    format!("[{}]", parts.join(", "))
}

fn fmt_vec(v: &[f64]) -> String {
    let parts: Vec<String> = v.iter().map(|x| format!("{x:.6}")).collect();
    format!("[{}]", parts.join(", "))
}

/// The file of `benchmarks-bounds` that mirrors an instance path.
fn reference_path(instance: &Path, dir: &str) -> Option<PathBuf> {
    let mut parts: Vec<_> = instance.components().collect();
    if parts.len() < 4 {
        return None;
    }
    let tail: PathBuf = parts.split_off(parts.len() - 4).iter().collect();
    let mut head: PathBuf = parts.iter().collect();
    head.pop(); // drop `bpmn-cpi-benchmarks`
    Some(head.join(dir).join(tail))
}

/// The `min` and `max` of every component of a `benchmarks-bounds` file.
fn parse_reference(text: &str) -> Option<Vec<(f64, f64)>> {
    let mut out: Vec<(f64, f64)> = Vec::new();
    let mut current: Option<(f64, f64)> = None;
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("- component:") {
            if let Some(c) = current.take() {
                out.push(c);
            }
            let _ = rest;
            current = Some((f64::NAN, f64::NAN));
        } else if let Some(rest) = t.strip_prefix("min:") {
            if let Some(c) = current.as_mut() {
                c.0 = rest.trim().parse().ok()?;
            }
        } else if let Some(rest) = t.strip_prefix("max:") {
            if let Some(c) = current.as_mut() {
                c.1 = rest.trim().parse().ok()?;
            }
        }
    }
    if let Some(c) = current {
        out.push(c);
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Compares `U` and `L` at the initial state, computed by the recursion of
/// Definition 12 alone, with the least and the greatest expected impact Storm
/// returned, over a whole list of instances in one process.
///
/// This is the check the exactness result asks for at scale. The recursion is
/// one traversal of the tree, so it runs on every instance of the grid,
/// including the ones no model checker can build, and comparing it with Storm
/// wherever Storm did answer is what says the two compute the same numbers.
fn cmd_sweep(args: &[String]) -> i32 {
    let args = match parse_args(args) {
        Ok(a) => a,
        Err(e) => return fail(&e),
    };
    let list = match args.positional.first() {
        Some(p) => p.clone(),
        None => return fail("give a file listing the instances"),
    };
    let text = match std::fs::read_to_string(&list) {
        Ok(t) => t,
        Err(e) => return fail(&format!("{list}: {e}")),
    };
    let paths: Vec<PathBuf> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect();
    let dir = args.flag("bounds-dir").unwrap_or("benchmarks-bounds").to_string();
    let threads: usize = args
        .flag("threads")
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);

    let next = std::sync::atomic::AtomicUsize::new(0);
    let agree = std::sync::atomic::AtomicUsize::new(0);
    let disagree = std::sync::atomic::AtomicUsize::new(0);
    let skipped = std::sync::atomic::AtomicUsize::new(0);
    let worst = Mutex::new((0.0f64, String::new()));
    let started = Instant::now();

    std::thread::scope(|scope| {
        for _ in 0..threads.max(1) {
            let (paths, dir, next, agree, disagree, skipped, worst) =
                (&paths, &dir, &next, &agree, &disagree, &skipped, &worst);
            scope.spawn(move || loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if i >= paths.len() {
                    return;
                }
                let path = &paths[i];
                let tree = match Tree::read(path) {
                    Ok(t) => t,
                    Err(_) => {
                        skipped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        continue;
                    }
                };
                let records = tables::Records::new();
                let store = Store::new(&tree, &records, 1);
                let b = bound::bounds(&tree, &store.engine.initial_state());
                let reference = match reference_path(path, dir) {
                    Some(p) => p,
                    None => {
                        skipped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        continue;
                    }
                };
                let theirs = match std::fs::read_to_string(&reference)
                    .ok()
                    .and_then(|t| parse_reference(&t))
                {
                    Some(v) if v.len() == tree.k => v,
                    _ => {
                        skipped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        continue;
                    }
                };
                let mut d: f64 = 0.0;
                for c in 0..tree.k {
                    d = d.max((b.lower[c] - theirs[c].0).abs());
                    d = d.max((b.upper[c] - theirs[c].1).abs());
                }
                // Storm prints ten significant digits, so anything below that is
                // agreement and not a difference.
                let scale = theirs
                    .iter()
                    .map(|(lo, hi)| lo.abs().max(hi.abs()))
                    .fold(1.0f64, f64::max);
                if d <= 1e-6 * scale {
                    agree.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                } else {
                    disagree.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                let mut w = worst.lock().unwrap();
                if d > w.0 {
                    *w = (d, path.display().to_string());
                }
            });
        }
    });

    let a = agree.load(std::sync::atomic::Ordering::Relaxed);
    let dis = disagree.load(std::sync::atomic::Ordering::Relaxed);
    let sk = skipped.load(std::sync::atomic::Ordering::Relaxed);
    let w = worst.lock().unwrap();
    println!("instances     {}", paths.len());
    println!("agree         {a}");
    println!("disagree      {dis}");
    println!("skipped       {sk}");
    println!("worst diff    {:.3e}  at {}", w.0, w.1);
    println!("seconds       {:.2}", started.elapsed().as_secs_f64());
    if dis == 0 {
        0
    } else {
        1
    }
}
