# strategy-determination-bpmn-cpi

**Pietro Sala** — Version 0.2

Material and code for the paper *On-the-Fly Strategy Synthesis for Expected
Impacts* (Chini, Amadori, Sala). The command-line tool `sdcpi`, Strategy
Determination for BPMN+CPI, lives at the root of this repository and is
documented below on its own terms; `examples/` holds two instances to
explore it with. `scripts/` holds the pipeline that redoes
the whole setup of the paper from nothing, documented in
`scripts/README.md`; the library asks nothing of the scripts, and the
scripts ask only the built library. The recorded rounds of the campaign are
still to be assembled; the repository is private until the paper is
submitted.

**Strategy Determination for BPMN+CPI**, a command-line tool. Given a business
process with decisions, chance and multi-dimensional costs, it answers one
question: is there a way of taking the decisions that keeps the expected cost
of the process under a budget, in every cost component at once, and if there
is, which one.

Rust, no dependencies:

```sh
cargo build --release
./target/release/sdcpi search examples/tiny.yaml --B 0.9,0.7
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
header keys above `tree:` are bookkeeping and optional, except that a file
may be named on the command line either by its path or, when a grid of
instances is laid out as `<root>/N-nested/M-independent/P-process_number/
D-mode.yaml`, by the key `N-M-P-D-mode` together with `--root <root>`.

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

## Commands

The core of the library is `search`; `info`, `bound` and `optima` are its
companions for exploring an instance; `verify`, `sweep` and `check` are
validation harnesses, there for the scripts and for a reader who wants to
distrust the implementation, and never needed to use the library.

```
sdcpi search  <instance> (--B a,b,... | --B-file F) [options]
                                             the tool: is there a strategy whose
                                             expected impact fits the budget, and
                                             which one

sdcpi info    <instance>                     what the file holds: sizes, counts
sdcpi bound   <instance>                     the two estimates at the start: one
                                             traversal of the tree, instant on any
                                             instance, every strategy enclosed
sdcpi optima  <instance>                     the exact least and greatest expected
                                             impact per component, over all
                                             strategies: walks every decision
                                             situation and may hit --max-states

sdcpi verify  <instance> [--bounds-dir DIR]  compare the optima against reference
                                             values stored on disk
sdcpi sweep   <listfile> [--threads N]       optima for every instance in a list
sdcpi check   <instance> [--alphas ...] [--ablations ...] [--cap N]
                                             cross-check the search against brute
                                             force on every stated configuration
```

## Options of `search`

| option | meaning |
|---|---|
| `--B a,b,...` | the budget `B`, one value per component |
| `--B-file F` | read `B` from a YAML file holding `B: [a, b, ...]` |
| `--workers N` | parallel workers over one search (default 1) |
| `--ablation MODE` | which of the two prunings to keep: `both`, `accept` (pessimistic only), `reject` (optimistic only), `none`; for measurement, the default `both` is the tool |
| `--selection MODE` | which open situation to extend next: `weighted` (drawn, favouring likely ones, the default), `uniform` (drawn evenly), `oldest` (deterministic, for runs that must repeat exactly) |
| `--seed S` | seed of the drawn selection |
| `--timeout SECS` | give up after this many seconds |
| `--epsilon E` | relative slack when comparing with `B`, for budgets produced by floating point |
| `--steal MODE` | how an idle worker finds work: from its ring neighbour (`ring`) or from anyone (`any`) |
| `--print-size 1` | on `yes`, print how large the returned strategy is: its situations, how many stay undecided, how many decisions it prescribes |
| `--print-strategy 1` | print the strategy itself; without this the decisions are not recorded, which is faster, and only the answer is reported |
| `--root DIR` | the grid directory, when the instance is a key |

## Output

Plain `key value` lines, one per row, made to be parsed as easily as read:
the instance, the budget `B` used, the answer, the seconds, and counters of
the work done (`expanded` situations, `choice states` met, `histories`
followed). With `--print-strategy 1` the decisions follow, one line per
decided situation.

## Exit

`0` on an answer either way, nonzero on a malformed instance or arguments.
