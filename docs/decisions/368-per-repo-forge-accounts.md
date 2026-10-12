---
id: ADR-368
title: "Per-repo forge accounts in the user config"
issue: 368
date: 2026-10-11
---

# ADR-368: Per-repo forge accounts in the user config

## Decision

Per-repo forge accounts in the user config (#368). `[forge.github.owners."<owner>"]` and `[forge.github.repos."<owner>/<repo>"]`, and the GitLab equivalents with nested groups on their full path, override the forge's default table; the most specific wins (repo, owner, default). Each takes exactly one of `token_command` and `token_env`. The owner and repo come from `origin`'s URL, SSH host aliases included, and apply to the forge `origin` is on. The chosen token is still resolved once and exported to every child, and a set `GH_TOKEN` still wins, now with a warning. `ns run` and `ns watch` check at start (not on `--dry-run`, and only for a forge with a table) that the token can read the repo, bounded to 15 s, and exit 2 naming the account and the table; a unit `ns watch` starts inherits the check through `NS_FORGE_CHECKED` and the settled token (no repeat warning) through `NS_FORGE_EXPORTED`, each set to `<forge>:<repo>` and honoured only for that forge and repo. `ns doctor` reports the account, the table, readability and the account `ssh -T` greets, and warns on a mismatch.

## Reason

Agreed in #368. One token per user broke an operator with a personal and a work account: a repo the other account owned returned 404, and the `NS_CONFIG` workaround was easy to forget in a shell or the systemd unit. User config only, so account names never land in a repo. Matching on `origin` rather than the checkout's path keeps the choice right in every worktree. The start check turns a night of 404s into one exit 2. An SSH mismatch only warns: git and the API may use different accounts on purpose. `NS_CONFIG` stays as the escape hatch. Spec: `cli/README.md` (`[forge]`), `docs/factory/starting-a-night.md` ("Before the first night").
