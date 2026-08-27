# scripts

The pipeline that produced the experimental section of the paper, stage by
stage. The library at the root of the repository stands on its own; these
scripts exist so that the whole setup of the paper can be redone from
nothing, and they write everything they produce at the root of the
repository, beside `src/`. Every script is deterministic under the seeds it
carries.

## Requirements

- Python 3 with PyYAML and plotly (`environment.yml` pins the versions).
- The library built at the root: `cargo build --release`.
- The source benchmark of the compliance problem (Workneh, Sala, Rizzi,
  Cristani, Information Systems 2025), cloned at the root as
  `process-impact-benchmarks/`: the one thousand process shapes and the
  generator of the impact vectors, called and not reimplemented.
- For the comparison stages only: Storm 1.13.0 and PRISM 4.10.1. The exact
  builds, sources and checksums used by the paper are recorded in the header
  comments of `compute_bounds.py` and `refinement_game.py`.

## The stages

1. **The instances.** `python3 scripts/make_bpmn_cpi.py --all --dimensions 2
   3 4 5 6 7 8 9 10 --mode all --seed 20260815` writes the grid,
   `bpmn-cpi-benchmarks/`, 54 000 instances: the thousand processes crossed
   with nine impact dimensions and six generation modes, completed with
   durations and with the two kinds of exclusive gateway in alternation.
2. **The box.** `python3 scripts/exact_optima.py` writes
   `benchmarks-optima/`: the least and the greatest expected impact per
   component of every instance, computed exactly by the library, the box the
   threshold questions bisect.
3. **The encodings and the reference bounds** (comparison only).
   `python3 scripts/to_prism.py` writes the model checker encodings, plain
   and with the decision trail; `python3 scripts/compute_bounds.py` asks
   Storm for the per-component bounds, the independent reference the optima
   are verified against.
4. **The games.** `python3 scripts/refinement_game.py --all --rounds 10`
   plays the ten threshold questions of every instance with the search and
   records every round under `benchmarks-refinements/`;
   `python3 scripts/replay.py --leader paco --follower storm --timeout 120`
   replays the same questions with Storm, and the same driver replays the
   three ablated configurations of the search.
5. **The numbers and the figures.** `python3 scripts/make_results.py`
   aggregates every recorded round into `results.json`, and
   `python3 scripts/make_figures.py` renders the figures of the paper from
   it into `figures/`. These two are the only scripts a reader needs in
   order to regenerate every number and every figure from the recorded
   rounds, without rerunning anything above them.
