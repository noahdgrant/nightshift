# Issue tracker: GitHub

Issues and PRs for this repo live on GitHub. Use the `gh` CLI for all operations.

## Conventions

- **Create an issue**: `gh issue create --title "..." --body "..."`.
- **Read an issue**: `gh issue view <number> --comments`.
- **Comment on an issue**: `gh issue comment <number> --body "..."`.

Infer the repo from `git remote -v`; `gh` does this automatically when run inside a clone.

## Linking issues

A PR body closes an issue with `Closes #<n>`.
