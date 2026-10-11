---
id: D18
title: "Skills are measured empirically"
date: 2026-10-08
---

# D18: Skills are measured empirically

## Decision

Skills are measured empirically. Each skill carries `evals/` with trigger cases (positive and negative) and behaviour cases (fixture repo, prompt, checks). `ns eval` runs each case several times with and without the skill, or old version against new, across configured harnesses, and reports pass rate, uplift, tokens and time. Online scorers grade real units with the same rubric format and feed `ns-improve`.

## Reason

From Google's skill evaluation practice and Warp's scorers. Fixture repos: one Python, one firmware (host build or emulator). Deterministic graders over `.ns/` artifacts where possible, LLM judge with a rubric otherwise. A skill whose ablation shows no uplift is a retirement candidate.
