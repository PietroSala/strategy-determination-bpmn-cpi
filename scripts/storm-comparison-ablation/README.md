# storm-comparison-ablation

The pipeline that produced the experimental section of the paper, the
comparison with Storm and the ablation of the search, stage by stage and
from scratch. The library at the root of the repository stands on its own
and knows nothing of what happens here; these scripts ask only the built
library, and everything they fetch or produce lands in this directory,
beside the scripts themselves.

Every stage is resumable: each one skips the work whose output already
sits on disk, so a stage that was interrupted is simply run again and
continues where it stopped.

## Requirements

- Python 3 with PyYAML and plotly (`environment.yml` pins the versions).
- The library built at the root of the repository: `cargo build --release`.
- For the comparison stages only: Storm 1.13.0 and PRISM 4.10.1. The exact
  builds, sources and checksums used by the paper are recorded in the header
  comments of `compute_bounds.py` and `refinement_game.py`.

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
