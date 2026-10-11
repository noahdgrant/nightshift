---
id: D10
title: "The ns CLI is written in Rust"
date: 2026-10-08
---

# D10: The ns CLI is written in Rust

## Decision

The `ns` CLI is written in Rust.

## Reason

Ships as one binary with no runtime, so any harness can call it. Control-CLI templates for target projects stay in the project's own language (Python for the examples).
