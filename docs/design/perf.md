# Performance budgets

The four existing SPEC limits pass on the current workload; the gate still exits 1 because SPEC has no limit for drawing a 10,000-line transcript. The draw clock runs from attach to the first full transcript frame, without waiting for stream-draw pacing. The earlier print-client and disk results below are historical comparisons.

All numbers use Cargo's release profile without overrides, as `cargo install` does. The files are `scripts/perf.py`, the final step of `scripts/gate.sh`, and the stand-in's request-arrival and reply-sent timestamps in `flows/standin.py`; their source lines count against SPEC's flows budget.

## Surface

```text
target=$(cargo metadata --no-deps --format-version 1 |
    python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')
python3 -m venv "$target/flows-venv"
"$target/flows-venv/bin/python" -m pip install -r flows/requirements.txt
RAZOR_BIN="$target/release/razor" "$target/flows-venv/bin/python" scripts/perf.py
```
```python
# flows/driver.py starts and stops the real processes and the model stand-in. After its PASS
# (no request failed, no process outlived the server), one line per budget; exit 1 if past or absent:
#   PASS|FAIL perf <budget>: <number> <unit> (<statistic>, <lowest> to <highest>); budget <limit>
#   FAIL perf Draw of a 10,000-line transcript: ...; budget none in SPEC.md
# The gate creates <target>/flows-venv and installs flows/requirements.txt there.
# One shared flows/terminal.py reads the app through a PTY and pyte's rendered cells.
# Budgets come from SPEC.md section 6, rows "| <budget> | <number> ms |" (or MiB), read the way
# size.py reads its rows: one definition, in the spec.

# flows/standin.py saves each model request's metadata as <n>.json, including two timestamps:
"arrived": float  # time.monotonic() once the request's headers were read, before its body
"sent": float     # time.monotonic() once the reply's last byte was written (model requests only)
```

## Data flow

One run builds a 10,017-line session and a second session with a busy job, then samples memory, terminal frames, and tool/turn overhead in that order. A run takes about four seconds.

1. `driver.flow` makes the scratch home under `/dev/shm` (the hard decision below), on the GPT route (`gpt-6.1-sol`, code mode off); the config adds one resident whose heartbeat is twelve hours away, so it is installed and never wakes.
2. A client call starts the server. The long session takes 63 turns, each a person's line, one `read` of a 100-line file and a 58-line answer: 10,017 transcript lines, 442 entries, a 1.9 MB record. The default `sessions.idle = 4` retains its conversation between turns, rather than reopening it for each input.
3. The busy session makes one `bash` call: `seq 2000000` (a 15 MB log), then a line every 10 ms until a stop file appears. It is working from before the first measurement to after the last. The loop sleeps: the load is the harness's own work for a busy session (writing the output file, the job-output tail refreshed every 250 ms, and the job's events), not a CPU hog on a shared box.
4. Idle memory: one second after the log is written (four tail updates), one sample.
5. Cold start: ten bare `razor` clients under a 120 × 40 pseudo-terminal, each timed from just before fork to receipt of the first synchronized frame showing `YOU` and the empty composer. They send no input and create no session.
6. Draw: ten new `razor --resume <long session>` clients, before the measured turns extend that transcript. Attach runs from just before fork to the first whole frame showing the last answer's last line and the composer; the initial empty frame does not count. The first history page draws immediately, like a key or resize, while streaming deltas keep the 83 ms pacing. The measurement includes the client's reads, layout and painting, not pyte's time to interpret the final frame. This is a viewport onto the 10,017-line session, with the product's bounded history page and mounted window, not 10,017 terminal rows painted at once.
7. Per call and per turn: ten turns on the long session (442 to 572 entries), each three `read` calls, one per response, then a three-line answer.
8. Every client quits with Ctrl-C; the PTY reader checks exit zero and exact restoration of terminal modes. The stop file ends the busy turn; the driver stops the server; then each number meets its budget.

## The budgets

Each definition below is the current clock. Only SPEC supplies enforceable limits; its transcript-draw limit is not written yet.

| Budget | From, to | In it | Not in it | Statistic |
| --- | --- | --- | --- | --- |
| Cold start to the first prompt | Just before the fork of bare `razor` under a PTY, to receipt of the first whole frame showing the composer. | Process start, the client's runtime, server discovery, terminal negotiation and the first screen. | Starting the already-running server; creating a session or sending a model request (bare Razor does neither until Enter); the terminal emulator's final-frame parsing. | Median of 10. |
| Draw of a 10,000-line transcript | Just before the fork of `razor --resume <id>`, to receipt of the first whole frame showing the last transcript line and the composer. | Client startup, API history/status attach, bounded transcript fold, layout and the complete visible frame. | Stream-draw pacing; scrolling through the whole record; terminal emulator parsing after receipt. | Median of 10. |
| Idle memory | One sample of the `razor serve` process's `RssAnon`. | The heap with what glibc keeps of freed memory, thread stacks as touched, anonymous maps. | File-backed pages, the binary's text most of them (8 to 12 MiB, varying with the filesystem the binary is read from, not with Razor's work; resident is printed beside it), the busy job's processes, live heap (no number for it exists without an allocator hook in the product; and what glibc keeps is memory the machine has given Razor, so it belongs in the count). | One sample: three taken 300 ms apart were equal. |
| Overhead per tool call | The last byte of a response holding one call, to the next request's arrival. | Reading the stream's end; recording the response, call and result; the `read` (microseconds); rendering the next request from the open conversation (about 2 MB, re-sent whole, as the Codex backend takes it); a TCP connection (the stand-in speaks HTTP/1.0). | The model's time; the stand-in reading the body. | Median of 30. |
| Overhead per turn | Just before a client sends the input, to the first request's arrival; plus the last reply's last byte, to `turn_end` reaching a client attached to the session. | The client's request, admission, the first render and send; recording the last response; `turn_end` and its sync; delivery on the stream. The turn's calls are the row above. | The model's time. | Median of 10. |

**Historical results, not the current workload.** On an earlier release build, the cold-start clock stopped at a print client's first model request, not a terminal frame. The memory-backed column summarizes ten runs; the disk column summarizes the earlier disk trials, including the 53 per-turn trials cited there. Disk trials used scratch edits to the script's temporary-directory setting; the checked-in run fixes it to `/dev/shm`. The machine was a shared multi-core Linux host under other load.

| Budget | Ten runs, scratch in memory | Earlier shared-disk trials | Original proposal |
| --- | --- | --- | --- |
| Cold start to the first prompt | 7.9 ms (7.6 to 8.3); worst sample 12.4 | 8.1 to 10.7 ms | 15 ms |
| Idle memory | 12.15 MiB (11.0 to 13.7); resident 22.7 to 25.8 | 11.3 to 14.0 MiB | 24 MiB |
| Overhead per tool call | 1.2 ms (1.0 to 1.5); worst sample 2.3 | 1.1 to 1.9 ms; worst sample 17.7 | 2.5 ms |
| Overhead per turn | 1.9 ms (1.5 to 2.2); worst sample 5.3 | 2.8 to 4.7 ms; 6 to 10 in 6 of 53 runs | 4 ms |

Headroom: about twice the median. Run to run the medians move 10 to 25%, and other tenants' load has doubled a median on this box; a budget at twice fails only a change that doubles the cost, which the re-parsed record and the held log each did in its red-proof below (blocking is the exception, said there).

## Current numbers

All four existing limits passed in all three runs. The draw row fails only because SPEC has no limit for it yet; that limit is to be written when the terminal app lands.

Measured 2026-10-28 with immediate first-history-page drawing, on the same machine. Three complete release runs each took ten cold starts and ten attaches. Each run kept the resident installed and the other session's 15 MB job busy. No compile ran during these measurements; the host remained shared.

| Budget | Median of the three run statistics | Lowest to highest run | Largest individual sample | SPEC limit |
| --- | ---: | ---: | ---: | ---: |
| Cold start to the first prompt | 8.4 ms | 8.3 to 8.5 ms | 11.8 ms | 15 ms |
| Draw of a 10,000-line transcript | 20.2 ms | 20.0 to 20.7 ms | 23.2 ms | Not written |
| Idle memory | 12.8 MiB | 12.5 to 14.7 MiB | One sample per run | 24 MiB |
| Overhead per tool call | 1.2 ms | 1.2 to 1.4 ms | 2.1 ms | 2.5 ms |
| Overhead per turn | 2.1 ms | 1.9 to 2.1 ms | 3.0 ms | 4 ms |

Before this change, ten runs at an earlier revision measured 101.1 ms for attach: the first empty frame's 83 ms pacing delayed the arriving history page. That page now draws immediately; streaming deltas retain their pacing. These timings cover only the terminal surfaces already built; they do not certify the deferred terminal surfaces, or complete-frame visual fidelity.

An earlier scratch 200 ms block at the start of the real draw function moved bare cold start to 212.1 ms and attach to 503.9 ms (ten samples each). Restoring the source and release binary returned the measurements to their normal range. This proves the clocks wait for actual rendered frames, not just a model request or an empty attach frame.

## What each budget catches: red-proofs

Re-parsing the record and holding a whole log in memory exceeded their limits; blocking did not reliably do so. These failure checks used scratch source mutations, built release, three runs each, then reverted (none committed):

| Mutation | Moved |
| --- | --- |
| Re-parsing the record: read and parse the whole record before every request (`responses/task.rs`, `request`) | per call 3.9 to 4.0 ms; per turn 4.5 to 4.7 ms; idle memory 24.3 to 27.0 MiB (the parse's freed memory, kept) |
| A log held in memory: the busy job's whole output re-read and kept at each tail update (`controller/session/actions.rs`, `tails`) | idle memory 40.3 to 42.6 MiB |
| Blocking in the loop: 100 ms of `std::thread::sleep` in each of the busy session's tail updates | no median moved; one run's worst cold start was 108 ms |

So blocking is caught only when it crosses sessions through shared state or is systematic. With a worker per core a blocked worker's tasks move to another, and a periodic block shows only in the worst samples, which every line prints and the gate does not hold: the worst sample swings two to three times with other tenants (clean worst per call: 1.4 to 17.7 ms).

## The hard decision: the scratch home in memory, not on the disk

On the disk, where Razor really runs, the numbers include the filesystem. Here that is a filesystem shared with other work on the machine: the `turn_end` sync took 0.5 ms or 8.5 ms depending on other tenants' writes, so the per-turn median was 6 to 10 ms in 6 of 53 runs and 2.8 to 4.7 otherwise. A budget above 10 ms misses the record re-parse (+2.5 ms per turn); one below it fails one gate in nine for nothing Razor did. In `/dev/shm` a sync costs what Razor's code costs: no run of this script has left 1.5 to 2.6 ms per turn (29 runs), and every spread narrowed. The cost: a change of sync policy (one per entry, say) is invisible to the gate; the run needs 117 MB of `/dev/shm`, kept there if the run itself fails, as the driver keeps a failed flow's scratch; and it is Linux-only, as the driver already is. Memory, then: none of SPEC's three killers is a disk's latency, and a gate that fails at random is a gate people stop reading.

The other workload choice is GPT rather than Claude, because it is Razor's code end to end. The Claude route's numbers are the SDK's Node processes: from the cold start to the first request 344 ms (median of five: spawning the bridge and the CLI), and four warm sessions held 648 MiB PSS (the bridge 92, each CLI 136, the server 10). Those would bury Razor's own share; they are a finding, not a budget.

## Findings

1. **Taken, second pass, 2026-10-28:** clients use a current-thread runtime (the terminal included), the server two workers, and `--version` no runtime. Five paired release runs of `scripts/perf.py`, alternating an earlier revision and the runtime change on this machine:

   | Budget | First version, 24 workers | Two server workers, current-thread clients |
   | --- | --- | --- |
   | Cold start | 8.3 ms (8.0–10.7) | 6.7 ms (6.5–7.4) |
   | Idle anonymous memory | 12.1 MiB (11.6–14.2) | 8.7 MiB (8.5–9.6) |
   | Per call | 1.4 ms (1.4–1.8) | 1.4 ms (1.3–1.7) |
   | Per turn | 2.2 ms (2.0–2.5) | 2.1 ms (2.1–2.4) |

   Cells are medians of five runs' statistics, with their range. Two workers keep another worker available when one blocks; one would trade that away for about 3 MiB. Four workers did not earn their memory: five exploratory runs used 11.5–13.7 MiB. A scratch mutation blocked one API worker for 100 ms on each status request, with a scratch client repeating every 250 ms during the busy-session measurements (five completed blocks per run, asserted nonempty). At 24/4/2 workers respectively, per-call medians were 1.4/1.2/1.2 ms and per-turn 2.2/1.7/2.0 ms; worst cold starts 10.0/9.5/10.3 ms. All four budgets passed in each run. This checks one periodically blocked worker, not arbitrary blocking or exhausted workers; the product still must not block. The mutation and client were removed. No allocator setting is shipped: the worker change alone removes a third of anonymous memory without a glibc-specific startup call.
2. **Taken, second pass:** `connect` probes immediately after spawning, then backs off from 1 ms to at most 100 ms, under one absolute 10-second timeout, retaining the refusal sentence. Ten fresh-home release starts went from 148.5 ms (146.8–153.5) to 50.8 ms (49.2–60.9) to the first model request; `razor devices`, which needs no memory lookup, from 108.8 ms (108.4–112.0) to 11.5 ms (10.1–16.3). The remaining ~40 ms before a first model request is finding 3. Three release `scripts/perf.py` runs after the backoff change passed every budget: cold-start medians 6.5–6.9 ms, memory 8.9–9.1 MiB, per call 1.3–1.4 ms and per turn 2.2–2.4 ms. An unavailable server still gives up at 10 seconds with the same sentence.
3. A directory's first session in a server's life runs `memoryd project resolve`: 43.6 ms to the first request, against 7.5.
4. On the Claude route, `sessions.idle` (4) is a memory setting: each warm idle Claude session holds a CLI of about 136 MiB.
