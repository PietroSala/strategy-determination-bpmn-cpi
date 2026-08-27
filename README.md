# strategy-determination-bpmn-cpi

**Pietro Sala** — Version 0.1

Material and code for the paper *On-the-Fly Strategy Synthesis for Expected
Impacts* (Chini, Amadori, Sala): the library implementing the search and the
two residual bounds, the generator that completes the compliance benchmark
with durations and with choices and nature nodes in alternation, the recorded
rounds of every configuration, and the scripts that regenerate every number
and every figure of the experimental section from them.

The layout so far: `sdcpi/` holds the library and its command-line tool,
documented standalone in `sdcpi/README.md`, and `examples/` two instances to
explore it with. The rest of the material is being assembled from the working
experiment repository; the repository is private until the paper is
submitted.
