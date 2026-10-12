---
id: D15
title: "Factory as code"
date: 2026-10-08
---

# D15: Factory as code

## Decision

Factory as code. A product's factory definition is a directory holding `nightshift.toml`, with `agents/`, `automations/`, `runners/`, `scorers/` and `skills/` beside it. A resource's name is its path. It lives at `.nightshift/` in a product repo, or at the root of its own repo when it spans several repos. Both are supported.

## Reason

Modelled on Warp Factories' definition files, without promising schema compatibility. The ns-* skills are the library; a definition pins a version of it and adds the product's roles, triggers, runners and scorers. `.nightshift/` is committed; `.ns/` is runtime and excluded.
