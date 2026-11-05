# Performance experiments

`perf/scenarios.py` measures an explicitly named Razor binary through the flows' real server,
client, tools and terminal. It extends the budget check in [perf.md](perf.md) with repeatable
experiments on ordinary disk. It is a separate command; the project gate still runs
`scripts/perf.py` with its existing workload and limits.

The scenarios cover short reads, shell commands, code execution, a long conversation, children
in a checkout or worktree, startup with stored sessions, and streamed text reaching a terminal
frame. Scenario differences are rows of data over shared drivers. `perf/workloads.py` owns the
long-conversation and busy-session definitions needed by both the budget check and this harness.
The harness does not start an unrelated busy session during measurements.

Use the release profile, the Python environment prepared for flows, and an output file outside
the checkout. Select the repository containing the flow helpers explicitly when measuring a
preserved binary from another checkout. The binary's hash identifies its bytes; its declared
build revision is recorded separately from the checkout revision. Record that revision from the
build, rather than assuming that the current checkout produced an older binary.

```sh
python perf/scenarios.py --binary /path/to/razor --binary-revision REV \
  --output /path/to/smoke.json --profile smoke
python perf/scenarios.py --binary /path/to/razor --binary-revision REV \
  --output /path/to/validation.json --validate
```

Use `--route claude` for the bridge and CLI route, `--scenario startup` to select a scenario,
and `--profile heldout` only after tuning. `--runs` repeats a scenario with fresh homes. The
binary must have its bridge bundle available as an ordinary install would. Both routes use the
same scenario shapes; the served model, SDK and CLI versions are read back into the result.

The scratch directory defaults to the operating system's ordinary temporary directory. Choosing
a memory filesystem requires an explicit scratch root. A fresh scratch home and server isolate
each scenario; these runs do not use an installed server or its sessions. Failed scenarios keep
their scratch directory, report failure, and make the command exit nonzero. Raw results belong
outside the repository. An explicit scratch root must leave room for the server's Unix socket
path; the command checks that before starting a scenario.

## What the clocks mean

All segment clocks use monotonic time in the Python process hosting the model stand-in. They
measure input submission to request arrival, the last response byte to the next request after
a tool, and the final response byte to the client's turn-end event. Child clocks cover launch,
the child's first request, its report reaching the parent, and settlement. Startup covers a
fresh client and server reaching their first model request, followed by warm session creation.
It does not imply a cold operating system file cache.

The stream clock runs from the stand-in's slice containing a word to receipt of the first
complete terminal frame showing it. It includes Razor's draw pacing and the terminal client's
work. The stand-in, API client, PTY reader and emulator share one Python process; its CPU is
reported so that measurement work is visible.

The stand-in replays response bytes over loopback. Endpoint emission and request handling have
their own clocks. These measurements establish local behavior under replay. They do not measure
a live provider or separate provider processing from network delay.

Each measured tool result must contain its expected complete output. Successful history setup
cannot satisfy that check. Requests must use the expected route and model, turns must finish
successfully, and child placement and completion must match the scenario. Counts, record sizes
and request sizes accompany the timings so doing less work cannot pass as an improvement.

## CPU and memory

Resource samples are taken at window edges, outside the timed interactions. They establish
per-process usage at those points, not the highest simultaneous memory of a process tree.

On Linux, the server's user and system CPU come from `/proc/PID/stat`, with the kernel's tick
precision recorded. Waited-child CPU is reported separately where available; it depends on the
kernel's propagation of reaped descendant usage. Per-thread `schedstat` supplies a finer CPU
counter, labelled as a lower bound if threads disappear. Memory fields are resident,
anonymous resident, the process lifetime resident high-water mark, and proportional resident
memory from `smaps_rollup` when readable.

On macOS, `ps` supplies each sampled process's accumulated CPU time at its displayed precision
and resident memory in KiB. Linux-only memory fields are absent. The active terminal client is
sampled while alive. Startup records each newly started server's usage separately. Descendant
snapshots name the processes present at an edge; short lived processes may be absent. A missing
whole-tree CPU or peak-memory total is unavailable, never inferred from those snapshots.

## Comparing candidates

Keep every raw run, including failures. Every sample records the binary hash, source and helper
hashes, workload, scratch filesystem, CPU and memory limits, load and compiler presence. Keep
the same build profile, limits, filesystem and work on both sides. Take at least five unchanged
pairs first to estimate noise; collect more when the interval cannot distinguish a useful effect.
Alternate A/B and B/A, using a fresh scenario home each time. Do not add background load.

Declare exclusions before sampling: technical failures, incomplete or different work, different
binaries than requested, changed limits or filesystem, compiler presence changing across the
pair's edges, or a load ratio above 1.5 invalidate a pair. Retain and report excluded pairs with
their reasons. Do not exclude a sample because its result is inconvenient.

A kept gain is at least 3%, its paired 95% interval excludes zero, a trace or profile explains
the reduced cost, behavior checks pass, and another runner reproduces it. Run the held-out
profile after tuning and check other metrics for regressions. Report inconclusive results when
contention prevents that conclusion. Three experiments with no win can retire an approach;
they do not finish a campaign with other material opportunities still open.

The controlled-delay scenarios check that added shell work and stream pauses move their own
segments by the expected amount. They validate the instruments; the campaign also checks a
deliberate slowdown in a scratch build of Razor. Neither check authorizes a relaxed budget.
