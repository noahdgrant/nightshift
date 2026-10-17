# Build order for the rebuilt core

Built from the `keep` rows of `FEATURES.md`. Each step ends with something that works and that you used by hand before it gets automated. A step does not start until the one before it has run on a real issue.

**Rule for everything marked `later`:** it comes back only when a real failure or cost, written down in an issue, needs it. The issue says what went wrong without it.

## 0. Where it lives

In `new/`, beside the old code. Nothing outside `new/` moves or changes, so the old code keeps working and stays the reference. `new/` takes pieces from the old code by copying them in, cut down, never by moving them. When `new/` can run the nightshift repo itself, it replaces the old code in one change.

## 1. Plumbing that already works

Keep as is: `.githooks` (fmt, clippy, test), `ci.yml`, `pr-title.yml`, `check-private.sh`, the label scheme, the disclosure line. Nothing to build.

## 2. By hand, no automation

Goal: a unit becomes a merged change request with you typing each phase into an interactive session.

1. `ns workspace new` and `remove` (with the `tree` fix folded in), `ns install`.
2. `docs/agents/*` for this repo, written by hand (stack, verify, tracker, labels, domain). Shrink `ns-setup` later from what you needed.
3. `ns-contract`, cut to: unit, workspace, the `.ns/<unit>/` artifacts, the disclosure line, gates.
4. `ns-build` with `ns-tdd`, then `ns-verify`, then `ns-ship`. Run each by hand on one small real unit.

Done when one unit has gone to a merged change request this way.

## 3. `ns run`, one unit

Goal: the same lifecycle, with the machine choosing the state.

1. The seams, each as a small interface with one implementation: harness (claude), forge (GitHub), vcs (git), runner (systemd, unused until `watch`), language (Rust pack).
2. A state table: read `.ns/<unit>/*.md`, pick the next phase, run it headless, stop at a gate.
3. `ns run` with `--gates stop` only, a retry limit and a run log.
4. `ns clean`.

Done when `ns run` takes the same unit as step 2 from brief to merge-ready change request with you approving at each gate.

## 4. Before the build: triage, define, plan

1. `ns-triage` cut down to: category, ready or not, the agent brief, the route.
2. Define: `ns-grilling` and `ns-requirements`.
3. Plan: `ns-design-doc` cut down to vertical slices.
4. Wire the routes into the state table.

Done when a vague unit reaches `ready-for-agent` with a brief and slices.

## 5. After the build: review

One reviewer, one pass, one fix cycle, `review.md`. No multi-provider, no quality records.

## 6. Learn from sessions

`ns learn` (the old `ns transcripts`) reads past sessions into a digest, and `ns-improve` turns it into a PR. This is the one feature that is new, not rebuilt.

## 7. Return trip: add `later` items in this order, each one only on a cited need

1. `ns watch --once` (brings back `memory`, `touches` and the runner)
2. `ns doctor`, `ns status`, `ns log`
3. `ns troubleshoot` and repro
4. auto-merge policy and the `human_review` globs
5. `ns eval`, `ns lint`
6. pinned skills and settings (`pin`)
7. the other rule packs, `ns quality`, `ns score`, `ns ratchet`, `ns rules`
