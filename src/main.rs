//! `sdcpi`: strategy determination for BPMN+CPI processes.
//!
//!     sdcpi determine <instance> (--B a,b,... | --B-file F) [options]
//!     sdcpi info      <instance>
//!     sdcpi bound     <instance>
//!     sdcpi optima    <instance>
//!
//! An instance is either a path to a YAML file or a key of the grid,
//! `<nested>-<independent>-<process_number>-<dimensions>-<mode>`, resolved under
//! `--root` (`bpmn-cpi-benchmarks` beside the executable tree by default).

mod arena;
mod parse;
mod bound;
mod exact;
mod search;
mod state;
mod tables;
mod tree;

use std::path::{Path, PathBuf};
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
        "determine" => cmd_determine(&args[1..]),
        "parse" => cmd_parse(&args[1..]),
        "bound" => cmd_bound(&args[1..]),
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

  sdcpi determine <instance> (--B a,b,... | --B-file F) [options]
  sdcpi parse     (<process> | --file F) [--out F]
  sdcpi info      <instance>
  sdcpi bound     <instance>
  sdcpi optima    <instance>

the grammar of parse, written inline or in the file
  process ::= region (, region)* | region op region      op ::= || | ^ | ^[p]
  region  ::= task | ( process )
  task    ::= ( name , duration )  |  ( name , duration , { name: value, ... } )
  -> sequence, || parallel, ^ a choice, ^[p] a nature node taking its left
  operand with probability p in (0,1); durations positive integers; the map
  names only the impacts that are strictly positive, {} and no map both
  meaning all zero; # comments to end of line. The instance file is written
  to standard output, identifiers in-order, impact_names in the order the
  names first appear, the vectors mapped by that order.

options for determine
  --B a,b,...           the budget B, one value per component
  --B-file F            read B from a yaml holding `B: [a, b, ...]`
  --workers N           number of workers (default 1)
  --ablation MODE       both | accept | reject | none      (default both)
  --selection MODE      weighted | uniform | oldest        (default weighted)
  --seed S              seed of the draw                   (default 20260816)
  --timeout SECS        give up after this many seconds
  --epsilon E           relative slack on the comparison with B (default 0)
  --steal MODE          ring | any                        (default ring)
  --print-size 1        on a positive answer print how large the partial
                        strategy is: the histories of the winning frontier, how
                        many of them are still open, and how many decisions it
                        takes. Neither the strategy nor its decisions are
                        printed
  --print-strategy 1    print the strategy at the end. Without it the decisions
                        are never recorded, which the search is faster for, and
                        only the outcome is reported
  --root DIR            where the grid sits, for a key rather than a path

options for optima
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
fn fail(message: &str) -> i32 {
    eprintln!("sdcpi: {message}");
    2
}

fn fmt_vec(v: &[f64]) -> String {
    let parts: Vec<String> = v.iter().map(|x| format!("{x:.6}")).collect();
    format!("[{}]", parts.join(", "))
}


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
fn cmd_parse(args: &[String]) -> i32 {
    let args = match parse_args(args) {
        Ok(a) => a,
        Err(e) => return fail(&e),
    };
    let text = match (args.flag("file"), args.positional.first()) {
        (Some(f), None) => match std::fs::read_to_string(f) {
            Ok(t) => t,
            Err(e) => return fail(&format!("{f}: {e}")),
        },
        (None, Some(_)) => args.positional.join(" "),
        _ => return fail("give the process inline or through --file, not both and not neither"),
    };
    match parse::to_yaml(&text) {
        Ok(p) => match args.flag("out") {
            Some(path) => match std::fs::write(path, &p.yaml) {
                Ok(()) => 0,
                Err(e) => fail(&format!("{path}: {e}")),
            },
            None => {
                print!("{}", p.yaml);
                0
            }
        },
        Err(e) => fail(&e),
    }
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
fn cmd_determine(args: &[String]) -> i32 {
    let args = match parse_args(args) {
        Ok(a) => a,
        Err(e) => return fail(&e),
    };
    let (_path, tree) = match load(&args) {
        Ok(v) => v,
        Err(e) => return fail(&e),
    };

    let from_file = match args.flag("B-file") {
        Some(p) => match bound_file(p) {
            Ok(v) => Some(v),
            Err(e) => return fail(&e),
        },
        None => None,
    };
    let threshold = match (
        from_file.as_ref().map(|v| v.as_slice()),
        args.flag("B"),
    ) {
        (Some(v), _) if v.len() == tree.k => v.to_vec(),
        (Some(v), _) => {
            return fail(&format!(
                "the bound file carries {} components against {} declared",
                v.len(),
                tree.k
            ))
        }
        (None, Some(t)) => match vector(t) {
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
        (None, None) => return fail("give --B or --B-file"),
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
    println!("B              {}", fmt_vec(&threshold));
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
// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------
