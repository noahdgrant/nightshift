# 03: Benchmark reserve and fulfil before and after the low-SKU set

Status: ready-for-agent
Category: enhancement
Blocked by: 02

## What to build

A benchmark script that times the median of 1000 `reserve` and 1000 `fulfil` calls on a 10,000-item store, before and after the low-SKU set from `docs/design/low-stock-alerts/v1.md`.

## Acceptance criteria

- [ ] The script prints both medians and the slowdown as a percentage.
- [ ] It exits 1 when either call is more than 10% slower, the limit in STOCK-3.
