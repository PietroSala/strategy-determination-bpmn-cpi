//! The grammar of a process, and the writer of the instance file.
//!
//!     process ::= task | "(" process op process ")"
//!     op      ::= "->" | "||" | "^" | "^[" prob "]"
//!     task    ::= "(" name "," duration ")"
//!             |   "(" name "," duration "," "{" impacts? "}" ")"
//!     impacts ::= name ":" number ("," name ":" number)*
//!
//! `->` is a sequence, `||` a parallel composition, `^` a choice and `^[p]` a
//! nature node whose left operand is taken with probability `p` in (0,1).
//! Every composition carries its own parentheses, so the term is read off the
//! string in one pass and no precedence has to be fixed. A duration is a
//! positive integer. The impact map of a task names only the impacts that are
//! strictly positive; a task may omit the map, or write `{}`, both meaning
//! every impact zero. The names are collected over the whole process in the
//! order of their first appearance, and that order is the meaning of the
//! vectors: `impact_names` in the emitted file lists them, and entry `i` of
//! every `impact` vector is the value of name `i`, zero where the task does
//! not name it. `#` starts a comment that runs to the end of the line.
//!
//! Identifiers are assigned by the in-order traversal, one-based: every node
//! of the low subtree numbers below its parent, every node of the high
//! subtree above, which is the numbering the reader of `tree.rs` checks.

/// One node of the parsed process, before identifiers exist.
enum Ast {
    Task {
        name: String,
        duration: i64,
        /// Indices into the collected name table, with the value of each.
        impact: Vec<(usize, f64)>,
    },
    Op {
        kind: &'static str,
        prob: Option<f64>,
        low: Box<Ast>,
        high: Box<Ast>,
    },
}

pub struct Parsed {
    pub yaml: String,
}

struct Scanner<'a> {
    text: &'a [u8],
    at: usize,
}

impl<'a> Scanner<'a> {
    fn new(text: &'a str) -> Self {
        Scanner { text: text.as_bytes(), at: 0 }
    }

    fn skip(&mut self) {
        loop {
            while self.at < self.text.len() && self.text[self.at].is_ascii_whitespace() {
                self.at += 1;
            }
            if self.at < self.text.len() && self.text[self.at] == b'#' {
                while self.at < self.text.len() && self.text[self.at] != b'\n' {
                    self.at += 1;
                }
            } else {
                return;
            }
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip();
        self.text.get(self.at).copied()
    }

    fn eat(&mut self, c: u8) -> Result<(), String> {
        self.skip();
        if self.text.get(self.at) == Some(&c) {
            self.at += 1;
            Ok(())
        } else {
            Err(format!(
                "expected {:?} at byte {}, found {:?}",
                c as char,
                self.at,
                self.text.get(self.at).map(|b| *b as char)
            ))
        }
    }

    fn name(&mut self) -> Result<String, String> {
        self.skip();
        let start = self.at;
        while self.at < self.text.len()
            && (self.text[self.at].is_ascii_alphanumeric() || self.text[self.at] == b'_')
        {
            self.at += 1;
        }
        if self.at == start {
            return Err(format!("expected a name at byte {start}"));
        }
        Ok(String::from_utf8_lossy(&self.text[start..self.at]).into_owned())
    }

    fn number(&mut self) -> Result<f64, String> {
        self.skip();
        let start = self.at;
        while self.at < self.text.len()
            && (self.text[self.at].is_ascii_digit()
                || matches!(self.text[self.at], b'.' | b'e' | b'E' | b'+' | b'-'))
        {
            // a minus is part of an exponent alone; the grammar has no
            // negative quantities
            if self.text[self.at] == b'-'
                && !matches!(self.text.get(self.at.wrapping_sub(1)), Some(b'e') | Some(b'E'))
            {
                break;
            }
            self.at += 1;
        }
        let s = std::str::from_utf8(&self.text[start..self.at]).unwrap_or("");
        s.parse::<f64>()
            .map_err(|_| format!("expected a number at byte {start}, found {s:?}"))
    }
}

fn parse_process(
    sc: &mut Scanner,
    names: &mut Vec<String>,
) -> Result<Ast, String> {
    sc.eat(b'(')?;
    // a task opens on a name, a composition on another parenthesis
    if sc.peek() == Some(b'(') {
        let low = parse_process(sc, names)?;
        sc.skip();
        let (kind, prob) = if sc.text.get(sc.at..sc.at + 2) == Some(b"->") {
            sc.at += 2;
            ("sequence", None)
        } else if sc.text.get(sc.at..sc.at + 2) == Some(b"||") {
            sc.at += 2;
            ("parallel", None)
        } else if sc.text.get(sc.at) == Some(&b'^') {
            sc.at += 1;
            if sc.text.get(sc.at) == Some(&b'[') {
                sc.at += 1;
                let p = sc.number()?;
                if !(p > 0.0 && p < 1.0) {
                    return Err(format!("a nature node carries {p}, and a probability lies in (0,1)"));
                }
                sc.eat(b']')?;
                ("nature", Some(p))
            } else {
                ("choice", None)
            }
        } else {
            return Err(format!("expected ->, ||, ^ or ^[p] at byte {}", sc.at));
        };
        let high = parse_process(sc, names)?;
        sc.eat(b')')?;
        return Ok(Ast::Op { kind, prob, low: Box::new(low), high: Box::new(high) });
    }
    // a task: name, duration, and an optional impact map
    let name = sc.name()?;
    sc.eat(b',')?;
    let d = sc.number()?;
    if d <= 0.0 || d.fract() != 0.0 {
        return Err(format!("the task {name} carries the duration {d}, and a duration is a positive integer"));
    }
    let mut impact: Vec<(usize, f64)> = Vec::new();
    sc.skip();
    if sc.peek() == Some(b',') {
        sc.eat(b',')?;
        sc.eat(b'{')?;
        if sc.peek() != Some(b'}') {
            loop {
                let iname = sc.name()?;
                sc.eat(b':')?;
                let v = sc.number()?;
                if v <= 0.0 {
                    return Err(format!(
                        "the impact {iname} of {name} is {v}, and the map names only what is strictly positive; leave a zero out"
                    ));
                }
                let idx = match names.iter().position(|n| n == &iname) {
                    Some(i) => i,
                    None => {
                        names.push(iname.clone());
                        names.len() - 1
                    }
                };
                if impact.iter().any(|(i, _)| *i == idx) {
                    return Err(format!("the task {name} names the impact {iname} twice"));
                }
                impact.push((idx, v));
                if sc.peek() == Some(b',') {
                    sc.eat(b',')?;
                } else {
                    break;
                }
            }
        }
        sc.eat(b'}')?;
    }
    sc.eat(b')')?;
    Ok(Ast::Task { name, duration: d as i64, impact })
}

fn count(ast: &Ast) -> (u32, u32) {
    match ast {
        Ast::Task { .. } => (1, 1),
        Ast::Op { low, high, .. } => {
            let (nl, tl) = count(low);
            let (nh, th) = count(high);
            (nl + nh + 1, tl + th)
        }
    }
}

fn fmt_number(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

fn emit(ast: &Ast, k: usize, next: &mut u32, indent: usize, out: &mut String) {
    let pad = "  ".repeat(indent);
    match ast {
        Ast::Task { name, duration, impact } => {
            let mut v = vec![0.0f64; k];
            for (i, x) in impact {
                v[*i] = *x;
            }
            let list: Vec<String> = v.iter().map(|x| fmt_number(*x)).collect();
            out.push_str(&format!("{pad}- task: {name}\n"));
            out.push_str(&format!("{pad}  id: {}\n", next_id(next)));
            out.push_str(&format!("{pad}  duration: {duration}\n"));
            out.push_str(&format!("{pad}  impact: [{}]\n", list.join(", ")));
        }
        Ast::Op { kind, prob, low, high } => {
            // in-order: the low subtree first, the node, the high subtree;
            // the node line prints before its children, so its identifier is
            // reserved by counting the low subtree first
            let (low_nodes, _) = count(low);
            let my_id = *next + low_nodes + 1;
            match prob {
                Some(p) => out.push_str(&format!("{pad}- {kind}: {p}\n")),
                None => out.push_str(&format!("{pad}- {kind}:\n")),
            }
            out.push_str(&format!("{pad}  id: {my_id}\n"));
            out.push_str(&format!("{pad}  low:\n"));
            emit(low, k, next, indent + 2, out);
            *next += 1; // the node itself, between the two subtrees
            out.push_str(&format!("{pad}  high:\n"));
            emit(high, k, next, indent + 2, out);
        }
    }
}

fn next_id(next: &mut u32) -> u32 {
    *next += 1;
    *next
}

/// Parses the grammar and writes the instance file, identifiers in-order and
/// one-based, `impact_names` in the order of first appearance.
pub fn to_yaml(text: &str) -> Result<Parsed, String> {
    let mut sc = Scanner::new(text);
    let mut names: Vec<String> = Vec::new();
    let ast = parse_process(&mut sc, &mut names)?;
    sc.skip();
    if sc.at != sc.text.len() {
        return Err(format!("trailing input at byte {}", sc.at));
    }
    if names.is_empty() {
        return Err("no impact is named anywhere; name at least one, the dimension being at least one".to_string());
    }
    let (nodes, tasks) = count(&ast);
    let k = names.len();
    let mut out = String::new();
    out.push_str(&format!("impact_names: [{}]\n", names.join(", ")));
    out.push_str(&format!("dimensions: {k}\n"));
    out.push_str(&format!("tasks: {tasks}\n"));
    out.push_str(&format!("nodes: {nodes}\n"));
    out.push_str("tree:\n");
    let mut next = 0u32;
    emit(&ast, k, &mut next, 1, &mut out);
    Ok(Parsed { yaml: out })
}
