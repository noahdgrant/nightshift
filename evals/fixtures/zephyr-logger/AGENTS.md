# zlogger

An out-of-tree Zephyr module: a sample ring buffer (`lib/ringbuf_log/`), a sensor reader behind a small HAL (`drivers/sensor_reader/`), and a demo app (`app/`) that logs simulated samples. Public headers are in `include/zl/`. Tests are ztest suites under `tests/`, run on `native_sim` with twister.

## Agent skills

Repo settings the `ns-*` skills read. Edit the files directly to change them.

- **Stack**: C on Zephyr 4.3; host tests via `scripts/twister.sh` (twister on `native_sim`). See `docs/agents/stack.md`.
- **Verify**: not set up yet. See `docs/agents/verify.md`.
- **Issue tracker**: local markdown under `.scratch/`. See `docs/agents/issue-tracker.md`.
- **Triage labels**: default. See `docs/agents/triage-labels.md`.
- **Domain docs**: single-context. See `docs/agents/domain.md`.
