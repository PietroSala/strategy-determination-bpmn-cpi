# storm-comparison-ablation

The pipeline that produced the experimental section of the paper, the
comparison with Storm and the ablation of the search, stage by stage and
from scratch. The library at the root of the repository stands on its own
and knows nothing of what happens here; these scripts ask only the built
library, and everything they fetch or produce lands in this directory,
beside the scripts themselves.

Every stage is resumable: each one skips the work whose output already
sits on disk, so a stage that was interrupted is simply run again and
continues where it stopped. Every stage is deterministic under the seeds
it carries, so what one machine produces, another reproduces. One caveat
belongs to history: the grid the campaign of the paper was drawn from
predates the stable seed derivation of stage 1, so stage 1 rewrites a
grid drawn the same way, not that grid; the grid of the paper ships in
`default_experiment/`.

## Replaying the recorded campaign

Every stage accepts `--replay-experiment`, which moves its data root:
`--replay-experiment <dir>` names any folder structured like this one
(`bpmn-cpi-benchmarks/`, `benchmarks-optima/`, `benchmarks-bounds/`,
`benchmarks-refinements/`, `results.json`), and `--replay-experiment`
alone names `default_experiment/`, the campaign of the paper as it was
recorded, which `setup.py` unpacks from the split archive shipped in
`default_experiment_archive/`. So

```sh
python3 scripts/storm-comparison-ablation/make_results.py --replay-experiment
python3 scripts/storm-comparison-ablation/make_figures.py --replay-experiment
```

regenerates every number and every figure of the paper from the recorded
rounds, and `replay.py --replay-experiment` re-asks the recorded questions
themselves, continuing from wherever the record stands; with
`--from-scratch` it first forgets every answer the follower has recorded,
within `--max-diagonal` when given, and re-asks from the beginning, the
questions of the leader never being touched. A stage pointed at an experiment writes there too, so copy
`default_experiment/` first if the shipped record should stay pristine.

## Requirements

- Python 3.10 or later with PyYAML, numpy, plotly and kaleido:
  `conda env create -f environment.yml`, or
  `pip install -r requirements.txt`.
- The library built at the root of the repository: `cargo build --release`.
- For the comparison stages only: Storm 1.13.0, the one external tool the
  pipeline runs. The exact build, source and checksum used by the paper are
  recorded in the header comments of `compute_bounds.py` and
  `refinement_game.py`. The models are written in the PRISM language, which
  Storm reads through its `--prism` flag; the PRISM tool itself is never
  invoked.

## The whole flow, from nothing

What follows is everything a new user runs, in order, to have the library
working and the experiments relaunched from scratch. The only fork is the
Python environment, and both ways lead to the same place.

First the repository and the library. The build needs Rust 1.80 or later
(`rustup` installs it in one line at rustup.rs):

```sh
git clone https://github.com/PietroSala/strategy-determination-bpmn-cpi.git
cd strategy-determination-bpmn-cpi
cargo build --release
./target/release/sdcpi determine examples/tiny.yaml --B 0.9,0.7
```

Then the Python environment, one of the two:

```sh
# the conda case
conda env create -f scripts/storm-comparison-ablation/environment.yml
conda activate sdcpi-experiments
```

```sh
# the pip case
python3 -m venv .venv
source .venv/bin/activate
pip install -r scripts/storm-comparison-ablation/requirements.txt
```

Then the setup, which clones the source benchmark at the pinned commit and
unpacks the recorded campaign:

```sh
python3 scripts/storm-comparison-ablation/setup.py
```

For the comparison stages, and only for them, install Storm 1.13.0 from
stormchecker.org so that `storm` is on the PATH.

From here the experiments relaunch from scratch by running the stages of
the next section in order, stage 1 to stage 5. Two honest warnings about
scale: the search stages take hours over the full grid, and the Storm
pass takes days, which is why every stage resumes when interrupted, and
why `--max-diagonal` and `--limit` exist for a first taste. And one
reminder from the stable seed: a grid rebuilt from scratch is drawn the
same way as the grid of the paper without being that grid, so numbers
compared against the paper come from the recorded campaign,

```sh
python3 scripts/storm-comparison-ablation/make_results.py --replay-experiment
python3 scripts/storm-comparison-ablation/make_figures.py --replay-experiment
```

while a run of the stages without flags measures your rebuilt grid on
your machine.

## The stages

Every command below is written from the root of the repository; the
scripts resolve their own paths, so the working directory does not matter.

0. **The source benchmark.** `python3 scripts/storm-comparison-ablation/setup.py`
   clones process-impact-benchmarks (Workneh, Sala, Rizzi, Cristani,
   Information Systems 2025), the one thousand process shapes and the
   generator of the impact vectors, called and not reimplemented, and pins
   it to the commit the paper used.
1. **The instances.** `python3 scripts/storm-comparison-ablation/make_bpmn_cpi.py
   --all --dimensions 2 3 4 5 6 7 8 9 10 --mode all --seed 20260815` writes
   the grid, `bpmn-cpi-benchmarks/`, 54 000 instances: the thousand
   processes crossed with nine impact dimensions and six generation modes,
   completed with durations and with the two kinds of exclusive gateway in
   alternation.
2. **The box.** `python3 scripts/storm-comparison-ablation/exact_optima.py`
   writes `benchmarks-optima/`: the least and the greatest expected impact
   per component of every instance, computed exactly by the library, the
   box the threshold questions bisect.
3. **The encodings and the reference bounds** (comparison only).
   `python3 scripts/storm-comparison-ablation/to_prism.py` writes the model
   checker encodings, plain and with the decision trail;
   `python3 scripts/storm-comparison-ablation/compute_bounds.py` asks Storm
   for the per-component bounds, the independent reference the optima are
   verified against.
4. **The games.** `python3 scripts/storm-comparison-ablation/refinement_game.py
   --all --rounds 10` plays the ten threshold questions of every instance
   with the search and records every round under `benchmarks-refinements/`;
   `python3 scripts/storm-comparison-ablation/replay.py --leader paco
   --follower storm --timeout 120` replays the same questions with Storm,
   and the same driver replays the three ablated configurations of the
   search.
5. **The numbers and the figures.** `python3
   scripts/storm-comparison-ablation/make_results.py` aggregates every
   recorded round into `results.json`, and `python3
   scripts/storm-comparison-ablation/make_figures.py` renders the figures
   of the paper from it into `figures/`. These two are the only scripts a
   reader needs in order to regenerate every number and every figure from
   the recorded rounds, without rerunning anything above them.
