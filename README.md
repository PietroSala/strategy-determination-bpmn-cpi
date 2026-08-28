# strategy-determination-bpmn-cpi

**Pietro Sala** — Version 0.2

The command-line tool `sdcpi`, Strategy Determination for BPMN+CPI,
lives at the root of this repository and is documented below on its own
terms; `examples/` holds two instances to explore it with.
`scripts/storm-comparison-ablation/` holds its test suite: a large
differential campaign that confronts the tool with the Storm model
checker and with three ablated configurations of its own search,
rebuildable from nothing, its own `README.md` walking the stages; the
library asks nothing of the scripts, and the scripts ask only the built
library. The recorded reference campaign ships as a split archive that
`setup.py` unpacks; the repository is private for now.

**Strategy Determination for BPMN+CPI**, a command-line tool. Given a business
process with decisions, chance and multi-dimensional costs, it answers one
question: is there a way of taking the decisions that keeps the expected cost
of the process under a budget, in every cost component at once, and if there
is, which one.

Rust, no dependencies:

```sh
cargo build --release
./target/release/sdcpi determine examples/tiny.yaml --B 0.9,0.7
```

## The model

A process is a YAML file holding a binary tree. The leaves are **tasks**, each
with a `duration` (how long it runs) and an `impact` (what it costs, a vector
of non-negative numbers: money, energy, hours, anything, one entry per
component; components are compared entry by entry and never added). The inner
nodes compose their two children:

| node | meaning |
|---|---|
| `sequence` | `low` runs, then `high` |
| `parallel` | both children run at the same time |
| `choice` | whoever runs the process picks one child |
| `nature: p` | chance picks: `low` with probability `p`, `high` with `1 - p` |

The smallest example, `examples/tiny.yaml`, is one coin flip between two
tasks:

```yaml
tree:
  - nature: 0.317537
    id: 2
    low:
      - task: T1
        id: 1
        duration: 2
        impact: [0.940423, 0.17498]
    high:
      - task: T2
        id: 3
        duration: 1
        impact: [0.830795, 0.799341]
```

The `id` fields number the nodes left to right and must be distinct; the
header keys above `tree:` are bookkeeping and optional. An instance is
always named on the command line by its path.

## Writing an instance

Instances can be written directly, in a grammar smaller than the YAML:

```
process ::= region ("," region)* | region op region
region  ::= task | "(" process ")"
op      ::= "||" | "^" | "^[" prob "]"
task    ::= "(" name "," duration ")"
        |   "(" name "," duration "," "{" name ":" value "," ... "}" ")"
```

A comma between regions is a sequence, as in the grammar of the model,
and it associates to the left: `A, B, C` means `(A, B), C`, the tree
staying binary, and the chain needs no parentheses of its own. The comma
is unambiguous: a task is recognised by the name after its parenthesis,
and the commas of the tuple are consumed inside it. `||` is a parallel
composition, `^` a choice and `^[p]` a nature node taking its left
operand with probability `p` in `(0, 1)`; each of these three carries
its own parentheses, one operator per pair, so no precedence exists to
remember, and mixing the sequence comma with another operator demands
the parentheses too. The outermost parentheses of the whole term are
optional. A duration is a positive integer. The map of a task names only the impacts
that are strictly positive: an absent name is zero, and a task may end at
the duration, or carry `{}`, both meaning every impact zero. `#` starts a
comment to the end of the line. The names are collected over the whole
process in the order they first appear; the emitted file lists them as
`impact_names`, and that order is the meaning of every `impact` vector and
of every budget `--B` you pass.

`examples/line.cpi` is an eight-task manufacturing line in this grammar, and
the whole flow is one pipe:

```sh
./target/release/sdcpi parse --file examples/line.cpi --out line.yaml
./target/release/sdcpi determine line.yaml --B 100,8
```

The budget may also be given by name, in any order, and it is rearranged
against `impact_names` before starting:

```sh
./target/release/sdcpi determine line.yaml --B '{hours: 8, kwh: 100}'
```

## Model checker encodings

`sdcpi to_prism` writes the instance as a PRISM `mdp` model, one guarded
command per move of the semantics, one bounded integer variable per node,
and one transition reward structure per impact component, named `impact0`,
`impact1` and so on, paid on the transitions that complete tasks. The model
loads in PRISM and in Storm, and the header of the emitted file carries the
query shapes to run against it.

There are two encodings, and the flag `--encode-history` picks one:

- `true`, the default, records in the state the branch taken at every
  choice and every nature node, written once and never cleared. The plain
  state forgets a decision once its region closes, so two runs that decided
  differently meet again, and a memoryless scheduler on the plain model
  cannot react to a closed decision. With the trail those runs stay apart,
  while the values and the probabilities do not move, so a memoryless
  (positional) deterministic scheduler of the model checker ranges exactly
  over the deterministic strategies of the instance, which is the class
  `determine` searches. This is the encoding for multi-objective queries
  restricted to pure schedulers.
- `false` emits the plain model, smaller, and right for every question
  about a single component, where a memoryless scheduler is already
  optimal.

```sh
./target/release/sdcpi to_prism line.yaml --out line.prism
```

`sdcpi to_objective` writes the question itself: the budget, in either form
`determine` accepts, becomes the property to run against that model, one
`R{"impactj"}<=Bj [ F n<root>=-2 ]` term per component joined in one
`multi(...)`, asking whether some scheduler keeps every expected total
reward within its bound by the time the root is done. Every value is kept
exactly as written, so `0.70` stays `0.70` and is never reformatted. Model
and property together are the whole handoff:

```sh
./target/release/sdcpi to_objective line.yaml --B '{kwh: 100, hours: 8}' --out line.props
storm --prism line.prism --prop line.props --multiobjective:purescheds positional --exact
```

## The question

Because chance is part of the model, a single run proves nothing; what the
decisions control is the **expected** impact, averaged over the draws of
`nature`. A **strategy** says which child to pick at every `choice`, and it
may decide differently depending on everything that happened so far. Given a
budget `B`, one value per component, the tool searches for a strategy whose
expected impact is at most `B` in every component. It answers `yes` with the
strategy, or `no`, meaning no strategy at all fits the budget.

The search works on the process directly, generating a situation only when
it reaches it, and it prunes with two estimates computed from the tree: an
optimistic one, the least any continuation can still cost, and a pessimistic
one, the most; a branch whose optimistic estimate already exceeds the budget
is dropped, and a branch whose pessimistic estimate fits is accepted without
being finished, which is why the returned strategy may leave some situations
undecided: any decision there fits the budget.

## Testing

Test suites live under `scripts/`, one folder with a `run.py` each:
`sdcpi test` lists them, `sdcpi test <suite> [options]` runs one, and
more suites will arrive as the library evolves. The dispatcher knows
only that contract, so the library stays free of what the suites
measure, and the options of a suite are listed by its own `--help`.

The suite included today confronts the built binary with the recorded
reference campaign:

```sh
./target/release/sdcpi test storm-comparison-ablation
```

The default run takes a few minutes: it unpacks the recorded campaign
into a fresh scratch folder, keeping the copy beside the scripts
pristine, and re-asks a small slice of the recorded questions through
the three ablated configurations of the search, which exercise the
binary just built, and through Storm, when it is on the PATH, which
referees the same questions from the outside. Every replayed round is
compared with the answer the campaign recorded, the exit code is zero
exactly when every round agrees, and `--full` extends the same
confrontation to the whole campaign.

## Commands

The core of the tool is `determine`; `info`, `bound` and `optima` are its
companions for exploring an instance.

```
sdcpi determine <instance> (--B a,b,... | --B-file F) [options]
                                             is there a strategy whose expected
                                             impact fits the budget, and which one

sdcpi parse     (<process> | --file F) [--out F]
                                             turn a process written in the grammar
                                             below into an instance file, on
                                             standard output or into --out

sdcpi to_prism  <instance> [--encode-history true|false] [--out F]
sdcpi to_objective <instance> (--B a,b,... | --B-file F) [--out F]
sdcpi mdp       <instance> [--max-states N] [--out F]
                                             dump the full single-step MDP of
                                             the instance, every state and
                                             every move, for inspection and
                                             for drawing
sdcpi test      [<suite>] [options...]      run a test suite from scripts/,
                                            forwarding the options; no suite
                                            lists the suites
                                             translate the instance into a PRISM
                                             model, on standard output or into
                                             --out; see below for the two
                                             encodings

sdcpi info    <instance>                     what the file holds: sizes, counts
sdcpi bound   <instance>                     the two estimates at the start: one
                                             traversal of the tree, instant on any
                                             instance, every strategy enclosed
sdcpi optima  <instance>                     the exact least and greatest expected
                                             impact per component, over all
                                             strategies: walks every decision
                                             situation and may hit --max-states
```

## Options of `determine`

| option | meaning |
|---|---|
| `--B a,b,...` | the budget `B`, one value per component, in the order of the instance |
| `--B '{name: value, ...}'` | the budget by name: the map is rearranged against the `impact_names` of the instance before starting, and it must give every impact exactly one value |
| `--B-file F` | read `B` from a YAML file holding `B: [a, b, ...]` or `B: {name: value, ...}`, on one line |
| `--workers N` | parallel workers over one search (default 1) |
| `--ablation MODE` | which of the two prunings to keep: `both`, `accept` (pessimistic only), `reject` (optimistic only), `none`; for measurement, the default `both` is the tool |
| `--selection MODE` | which open situation to extend next: `weighted` (drawn, favouring likely ones, the default), `uniform` (drawn evenly), `oldest` (deterministic, for runs that must repeat exactly) |
| `--seed S` | seed of the drawn selection |
| `--timeout SECS` | give up after this many seconds |
| `--epsilon E` | relative slack when comparing with `B`, for budgets produced by floating point |
| `--steal MODE` | how an idle worker finds work: from its ring neighbour (`ring`) or from anyone (`any`) |
| `--print-size 1` | on `yes`, print how large the returned strategy is: its situations, how many stay undecided, how many decisions it prescribes |
| `--print-strategy 1` | print the strategy itself; without this the decisions are not recorded, which is faster, and only the answer is reported |

## Output

Plain `key value` lines, one per row, made to be parsed as easily as read:
the instance, the budget `B` used, the answer, the seconds, and counters of
the work done (`expanded` situations, `choice states` met, `histories`
followed). With `--print-strategy 1` the decisions follow, one line per
decided situation.

## Exit

`0` on an answer either way, nonzero on a malformed instance or arguments.

## References

The problem the tool decides, the synthesis of a strategy whose expected
impact stays within a budget over processes with choices, probabilities
and multi-dimensional impacts, was introduced in [1]. The Petri net
semantics behind the model, and the PACO tool family this library
belongs to, are presented in [2]; `sdcpi` implements the synthesis on
the fly, exploring the process without ever building its state space,
and writes the encodings and the objectives for the Storm model checker
[3]. The process shapes and the impact modes of the campaign come from
the benchmark of [4], and the underlying formalism of strategies and
expected values is that of Markov decision processes [5].

1. E. Chini, P. Sala, A. Simonetti, O. Zare. Reactive synthesis for
   expected impacts. Electronic Proceedings in Theoretical Computer
   Science 409, pages 35–52, 2024.
   [doi:10.4204/EPTCS.409.7](https://doi.org/10.4204/EPTCS.409.7)
2. E. Chini, D. Amadori, P. Sala, S. N. Rajput, M. Baldi,
   M. Cappelletti. PACO: a Petri net based tool for designing,
   simulating, and analyzing multi-objectives stochastic processes.
   Application and Theory of Petri Nets and Concurrency, Springer, 2026.
   [doi:10.1007/978-3-032-27879-1_16](https://doi.org/10.1007/978-3-032-27879-1_16)
3. C. Hensel, S. Junges, J.-P. Katoen, T. Quatmann, M. Volk. The
   probabilistic model checker Storm. International Journal on Software
   Tools for Technology Transfer 24(4), pages 589–610, 2022.
   [doi:10.1007/s10009-021-00633-z](https://doi.org/10.1007/s10009-021-00633-z)
4. T. C. Workneh, P. Sala, R. Rizzi, M. Cristani. Business process
   compliance with impact constraints. Information Systems 129, 102505,
   2025.
   [doi:10.1016/j.is.2024.102505](https://doi.org/10.1016/j.is.2024.102505)
5. M. L. Puterman. Markov Decision Processes: Discrete Stochastic
   Dynamic Programming. Wiley, 2014.
