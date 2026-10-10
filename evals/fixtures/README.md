# Eval fixtures

Starting repos for `ns eval` cases. See `docs/EVALS.md` for the format. Each trial copies only the fixture directory, so this file is never visible to the agent under test. Keep seeded defects documented here and nowhere inside a fixture.

## py-inventory

Pure-Python stock and reservation package: `src/inventory/` (Warehouse, Reservation, StockLevel, JSON storage, argparse CLI), 24 passing tests under `tests/`, `docs/agents/` filled in for the local `.scratch/` tracker (default triage labels), `GLOSSARY.md` at the root, verify not set up.

Run the suite from the fixture root with `python3 -m pytest -q` (pytest only; `pyproject.toml` sets `pythonpath = ["src"]`).

### Seeded defects

**(a) Exact-capacity reservation rejected.** `Warehouse.reserve` in `src/inventory/warehouse.py` checks `if quantity >= available:` instead of `>`. Reserving exactly the available quantity raises `InsufficientStock` ("cannot reserve 5 of 'X': only 5 available"). Reserving less works, and reserving more is correctly rejected. Fix: `quantity > available`. The CLI shows it too: `reserve SKU <all available>` exits 1. Used by `ns-tdd/evals/cases/py-capacity-off-by-one` and, as a unit brief, `ns-build/evals/cases/py-bug-fix-exact-capacity`.

**(b) Expired reservations counted in stock levels.** `Warehouse.stock_levels(now)` in `src/inventory/warehouse.py` sums every held reservation per SKU without checking expiry, so `StockLevel.reserved` and `.available` (and the CLI `status` output) still include expired reservations. `Warehouse.reserved()`/`available()`/`reserve()` go through `_active()` and are correct, so the two paths disagree after a reservation expires. Root-cause fix: make `stock_levels` use the same active-reservation logic (`self.reserved(sku, now)` or `_active`), not a second copy of the filter.

**(d) Multi-part SKUs rejected.** `Item.__post_init__` in `src/inventory/models.py` accepts only SKUs matching `[A-Z]+-[A-Z0-9]+`: exactly two parts, the first letters only. `GLOSSARY.md` defines a SKU as one or more parts of uppercase letters and digits, joined by single hyphens. So `BOLT-M6-40`, `WASHER` and `M6-40` are refused with `ValueError("invalid sku ...")` (CLI: `add-item` exits 1). Root-cause fix: make the pattern the glossary rule, `[A-Z0-9]+(-[A-Z0-9]+)*`. Allowing only the reported shape (an optional third part, or the reported SKU itself) is the special-case fix, and dropping the check lets malformed SKUs through. Used by `ns-tdd/evals/cases/py-special-case-fix`.

No existing test covers defects (a), (b) or (d): no test reserves exactly the available quantity, the only `stock_levels` test uses unexpired reservations, and the only SKU-rule test rejects a lowercase SKU.

### Room for a feature

- `Warehouse.release_expired(now) -> list[Reservation]`: does not exist. Expired reservations stay in `reservations()` (and in the state file) until cancelled. Used by `ns-tdd/evals/cases/py-release-expired-feature`.
- `status --json` in the CLI: does not exist in the fixture. The `ns-verify` cases `py-status-json-*` commit a build of it in setup (`build.patch` in the case dir), with a brief of three criteria: every item as JSON in SKU order (AC1), only the named SKU (AC2), and an unknown SKU exits 1 (AC3).

### Room for a refactor

`run()` in `src/inventory/cli.py` dispatches every command through one if/elif chain on `args.command`. Splitting it into one handler per command is a pure refactor with two easy ways to change behaviour. `status` sets `changed = False` so it never saves: run against a missing state file, it must not create one. And `run(args, out=None)` writes to the stream it is given, so a handler that prints to `sys.stdout` breaks callers that pass `out`. The existing tests cover neither, and none cover `cancel`, `fulfil` or the `add-item` and `receive` output. Used by `ns-tdd/evals/cases/py-refactor-keep-behaviour`.

### Seeded defects in case builds

**(c) `status --json` ignores the SKU.** The build in `ns-verify/evals/cases/py-status-json-misses-sku-filter/build.patch` checks that the SKU exists but prints every item under `--json`, so AC2 fails while AC1 and AC3 hold. Its test for AC2 stocks a single item, so the suite passes and the seeded `build.md` says `status: pass`. Only driving the CLI with two or more items shows the defect. The sibling case `py-status-json-all-criteria-met` commits the correct build with the same brief, `build.md` and test names.

**(e) `renew` reads the wall clock.** `ns-tdd/evals/cases/py-reservation-expiry-seam` applies `renew.patch` in setup, so the build sits in the start commit. It adds `Warehouse.renew(reservation_id, ttl)` and a CLI `renew ID [--ttl MINUTES]`, with three error-path tests (an unknown id through the class and the CLI, a non-positive ttl). `renew` calls `datetime.now()` itself, both to check expiry and to set `expires_at = now + ttl`, so the CLI's `--now` never reaches it. A replay at a past `--now` refuses a live hold as expired and prints a real-time expiry; one at a future `--now` renews a lapsed hold. Root-cause fix: `renew` takes `now` like every other time-dependent call, and the CLI passes it. Dropping the expiry check, or testing with `time.sleep` and `datetime.now()`, leaves the wall clock in place and fails the case's hidden test, which drives the CLI on a 2020 timeline and a 2099 one. Validated by hand: the unfixed build and that naive fix fail, and the root-cause fix with a fixed-time test passes every check.

### Notes for case authors

- Avoid exact-capacity reservations, `stock_levels` after expiry, and SKUs other than two parts (letters, then letters or digits, like `BOLT-M6`) in hidden acceptance tests unless the case is about that defect, or the test will fail for an unrelated reason.
- The local tracker in `.scratch/` is empty except for `.gitkeep`. Triage cases add issues through their `files/` overlay at `.scratch/<feature>/issues/<NN>-<slug>.md`, with `Status:` and `Category:` lines near the top.

## zephyr-logger

Out-of-tree Zephyr module named `zlogger` (`zephyr/module.yml`, root `CMakeLists.txt` and `Kconfig`). It holds a sample ring buffer in `lib/ringbuf_log/` (`CONFIG_ZL_RINGBUF_LOG`), a sensor reader behind a function-pointer HAL in `drivers/sensor_reader/` (`CONFIG_ZL_SENSOR_READER`), a demo app in `app/`, and public headers in `include/zl/`. There are two ztest suites on `native_sim`: `tests/lib/ringbuf_log/` (11 cases) and `tests/drivers/sensor_reader/` (6 cases, fake HAL with a fake clock). All 17 pass. `docs/agents/` is filled in for the local `.scratch/` tracker with default triage labels, and verify is not set up. The fixture has no `GLOSSARY.md`.

Requires the `zephyr` capability. Run the suites from the fixture root with `scripts/twister.sh`, which wraps `"$ZEPHYR_BASE/scripts/twister" -T tests/ -p native_sim -x=ZEPHYR_EXTRA_MODULES="$PWD" -O "$(mktemp -d)" --inline-logs`. Measured against Zephyr v4.3.0: about 23 s for both suites from cold, 12-16 s for one suite. `python3` on `PATH` must have Zephyr's `scripts/requirements-base.txt` installed. Each test `CMakeLists.txt` appends its own module root to `ZEPHYR_EXTRA_MODULES` and aborts if `zlogger` resolves anywhere else, so a run can't silently build another copy.

### Seeded defect

**Drop-oldest overflow neither drops nor caps.** In `zl_ringbuf_put` (`lib/ringbuf_log/ringbuf_log.c`), the full-buffer branch for `ZL_RB_DROP_OLDEST` only does `rb->dropped++`. It never advances `rb->tail` and never decrements `rb->count`. After one put into a full buffer, `zl_ringbuf_count` reports capacity+1. The new sample has overwritten the slot at `tail`, so the next `zl_ringbuf_get` returns the newest sample instead of the oldest one still kept (with capacity 4 and puts 1..5, `get` returns 5 rather than 2). Later puts also stop dropping, because `count == capacity` is never true again. Fix: in that branch, `rb->tail = advance(rb, rb->tail); rb->count--;` before the write. A fix that clamps `zl_ringbuf_count` or changes `get` to compensate is a symptom patch.

No existing test overflows a drop-oldest buffer. `test_drop_oldest_accepts_up_to_capacity` stops at exactly capacity, and the sensor-reader suite uses `ZL_RB_REJECT_NEW`. Verified: adding a full+drop test makes `lib.ringbuf_log` fail (`zl_ringbuf_count(&rb) not equal to CAPACITY`), and the two-line fix makes it pass.

### Room for a feature

- **Read timeout on the sensor reader.** `zl_sensor_reader_sample` returns `-EAGAIN` immediately when `data_ready` reports nothing. A `zl_sensor_reader_sample_timeout(reader, timeout_ms)` that polls until ready or until `hal->uptime_ms` passes the deadline (`-ETIMEDOUT`) doesn't exist yet. The HAL already has the clock, so the fake can drive it. Used by `ns-tdd/evals/cases/zephyr-sensor-fake-hal`, whose hidden suite spends 10 ms of fake HAL time per poll (1 ms in one test) and never moves the kernel clock. A deadline on `k_uptime_get` or a fixed poll count (`timeout_ms / 10`) fails it, and checks reject kernel clock reads in `drivers/` and `include/` and sleeps or kernel clock reads in the added tests. Validated by hand: the unfixed fixture, the kernel-clock fix tested with `k_msleep`, and the fixed-count fix fail, and a HAL-clock fix with a fake-clock test passes every check.

### Notes for case authors

- Hidden acceptance suites can live in a temp dir with a plain `CMakeLists.txt` (no self-pinning) and run with `-x=ZEPHYR_EXTRA_MODULES="$PWD"`. See `ns-tdd/evals/cases/zephyr-ringbuf-drop-oldest`.
- Twister output must go outside the repo (`-O` a temp dir), or it pollutes `diff_scope`.
- `scripts/twister.sh <files>` maps files to suites, so `fails_on_base` can pass it `{changed_tests}` directly.
