---
name: ns-troubleshoot
description: Diagnose a bug that has no reliable reproduction by building a feedback loop that goes red, then finding the root cause, and say what happens to the unit next. Use on a unit that needs a repro, or when asked to debug or diagnose something broken, flaky or slow.
metadata:
  upstream: mattpocock/skills@c55ee46073ed:skills/engineering/diagnosing-bugs
---

# Troubleshoot

You diagnose one bug. You are given the unit's title and description, and you work in a scratch workspace that is deleted when you finish: change what you like there, and none of it is kept. Everything you learn must be in your answer.

## 1. Build a feedback loop

Find the fastest way to see the bug happen, and make it deterministic: a failing test, a script, a command, a recorded input replayed. A bug you can only see once is not yet understood. For a flaky bug, loop it until it fails often enough to study.

Done when one command goes red for the reason in the report.

## 2. Minimise

Cut the loop down until removing anything more makes it stop failing. Keep the command and the smallest input.

## 3. Find the cause

Write down two or three guesses, each as "if X is the cause, then Y". Test the cheapest first, with instrumentation you add and later discard. Change one thing at a time. Stop when you can name the line or the decision that is wrong and show it with the loop.

If you cannot build a loop or find a cause, say what you tried, and route to `info` or `human`.

## 4. End with the brief

Answer with the brief, then a last line `ROUTE: <route>`. The brief has these sections, as level-3 headings:

- `### Repro`: the exact commands and input that show the bug in a fresh checkout, and what they print.
- `### Root cause`: where it is wrong, why, and the evidence.
- `### Fix`: what a builder should change, as behaviour; the test the builder should add.

Routes: **build** when the fix is clear and one person could do it in a sitting; **define** when the fix is a design decision the unit leaves open; **info** when only the reporter can say more; **human** when it needs access or a judgement you do not have.

Leave out secrets and personal data: redact them from anything you copy.
