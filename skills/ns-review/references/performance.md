# Performance reviewer

Find changes that make the system measurably slower, larger or hungrier. Quantify when you can: "adds one query per item, ~50 ms each at 100 items" beats "could be slow". Leave micro-optimisations alone unless the code is on a hot path.

## General

- **N+1 patterns**: a query, request or file read inside a loop over results.
- **Unbounded work**: loops, reads or fetches with no limit; list endpoints with no pagination; reading a whole file or response into memory when streaming works.
- **Blocking calls** on an async event loop or request path (`time.sleep`, sync I/O inside `async def`).
- **Algorithmic cost**: quadratic scans where a set or dict lookup works, repeated sorting, repeated parsing of the same input.
- **Allocation in hot paths**: large objects or copies created per call or per item.
- **Caching**: a missing cache on an expensive repeated call, or a cache with no bound or invalidation.
- **Independent work serialised** for no reason.

## Firmware

- **ISR latency**: interrupt handlers do the minimum (capture, flag, defer). Flag logging, `printf`, allocation, floating point on cores without an FPU, long loops, or blocking calls inside an ISR. Check interrupt priorities and how long interrupts are disabled in critical sections.
- **Stack usage**: large local buffers, recursion, deep call chains, especially in ISRs and small RTOS task stacks. Ask for the stack-usage report or high-water mark when the diff grows a task's frame.
- **Heap usage**: dynamic allocation after init, fragmentation from varied sizes, allocation that can fail with no handling. Many firmware standards forbid heap use after startup.
- **Flash and RAM budget**: the diff's growth in `.text`, `.data` and `.bss`. Compare the linker map or size report before and after when `docs/agents/stack.md` names one. New large tables or string literals belong in flash, not RAM.
- **Power**: busy-wait loops where a sleep or interrupt works, peripherals left clocked or powered, higher polling rates, radio kept on longer, wake-ups added to the idle path.
- **Blocking calls in hot paths**: polling a peripheral flag without a timeout, blocking UART or I2C transfers in the main loop or a high-priority task, delays inside control loops.
- **Timing**: deadline-bound code (control loops, protocol timing) given new work without a measurement. A timing claim with no measurement is `inconclusive`.
