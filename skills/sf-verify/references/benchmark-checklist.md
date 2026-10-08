# Benchmark checklist

Use this for every number that goes into evidence: a before and after, a regression claim, a size or memory budget, a latency, a current draw. [Explain the number](../../sf-principle-explain-the-number/SKILL.md) says why. Answer each question with evidence from a run, not from a guess about the code.

For a quick ballpark the human asked for, one run is enough. Still check questions 4 and 7, and say it was one run. A choice between options is never a ballpark.

## Before you run anything

- Write down the claim you expect to make, in the words you would ship ("export is 30% faster at p50 on the 60k-row dataset"). The questions test that sentence.
- Read the measurement script. Note what it times, what it counts, and what it ignores.
- Check the load average with `uptime` and the core count with `nproc`. If the machine is busy, find out what is running. If you can't stop it, interleave the sides so both see the same noise, and say so in the report.

## The questions

1. **Why not double?** Name the limiter. Profile in a run you don't report, because profilers slow the work down. Use CPU per process (`top`, `pidstat`), a profiler for the runtime (`py-spy`, `cProfile`, `perf`), I/O wait, and syscall counts (`strace -c`). Map the hot spot to source. Watch the load generator too: if it saturates first, you measured the load generator. If a change didn't move the number, the limiter explains why.
2. **Was it tuned?** Run every side the way production runs it: release builds, production flags and env, batching and transaction settings, pools, caches as warm or cold as production sees them, the same versions and data. If one side runs on defaults, you compared configurations, not implementations. Tune it and measure again before you pick a winner.
3. **Did it break limits?** Do the arithmetic. Compare bytes per second with disk and network bandwidth, and operations per second times cost per operation with the cores you have. Removing a piece that takes 10% of the run makes the run at most about 11% faster. A result past a limit means the run measured something else: a cache, a no-op, a bug.
4. **Did it error?** Count failures and non-success responses, and check the outputs are correct, not just present. Rejections are often fast, and timeouts and retries are slow. If the script doesn't count errors, add the count.
5. **Does it reproduce?** Run each side at least 5 times, alternating (A, B, A, B, ...) so warmup, caches and drift don't favor one side. Report the median and the range. A gap smaller than the run-to-run spread is no measurable difference. When the call is close, use a rank-sum test or the harness's own statistics.
6. **Does it matter?** Next to any micro result, measure the end-to-end path a user waits on, with realistic data sizes and concurrency. Report the micro result as a share of the whole.
7. **Did it even happen?** Confirm the work ran inside the timed region: the request reached the server, the rows were written, the bytes were read, the result was used. Generators nobody iterates, coroutines nobody awaits, and timeouts all produce numbers for work that never happened.

## Firmware notes

- **Why not double**: name the bound (CPU cycles, bus or DMA bandwidth, flash wait states, a peripheral clock, the UART baud). Measure with the cycle counter (DWT `CYCCNT` on Cortex-M) or a GPIO toggle on a logic analyzer, not with a log line, which costs more than many ISRs.
- **Tuned**: compare the build profile and optimization level that ships, with the same clock tree and the same power mode. A debug build or a different `-O` level is a different configuration.
- **Limits**: check against the clock rate, the bus width and the peripheral's datasheet maximum.
- **Reproduce**: power state, temperature and supply voltage drift between runs. Interleave the images, flashing each in turn, rather than running all of A then all of B.
- **Size budgets**: read flash and RAM use from the linker map or the `size` output of the shipped build, against the budget in `stack.md`.
- **Power**: report mean current, peak current and energy over a stated window, with the raw trace path, and the firmware state during the window.

## Report

- Lead with the verdict: faster, slower, no measurable difference, or inconclusive.
- Give the number with its unit, the run count, the range, and the limiter. For example: "p50 41 ms to 33 ms, median of 7 runs per side, range 32 to 35 ms after, bound by JSON parsing on one core."
- The verdict is inconclusive when you claim a difference but can't name the limiter, when a side ran untuned, or when you couldn't check questions 4 and 7. Name the gap.
- In `evidence.md`, put the primary number under Numbers and the raw runs in an artifact under `.sf/<unit-id>/evidence/`.
