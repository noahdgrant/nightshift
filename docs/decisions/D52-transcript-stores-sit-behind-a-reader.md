---
id: D52
title: "Transcript stores sit behind a Reader trait with a size+mtime index"
date: 2026-10-10
---

# D52: Transcript stores sit behind a Reader trait with a size+mtime index

## Decision

Transcript stores sit behind a `Reader` trait (`locate`, `load`, `entries`, `in_project`); `ns transcripts` lists sessions from a size+mtime index at `$XDG_DATA_HOME/nightshift/transcripts-index.json`, keyed `<source>:<store path>/<id>`.

## Reason

`locate` returns size and mtime from directory metadata, so `--new` and `--since` skip files before any is opened. `in_project` takes the user's path and each reader owns the matching, so Codex and Cursor readers don't inherit Claude's slug model.
