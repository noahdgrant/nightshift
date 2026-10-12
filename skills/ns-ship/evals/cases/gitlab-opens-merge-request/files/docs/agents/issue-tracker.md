# Issue tracker: GitLab

Issues and merge requests for this repo live on GitLab. Use the `glab` CLI for all operations.

## Conventions

- **Create an issue**: `glab issue create --title "..." --description "..."`.
- **Read an issue**: `glab issue view <number> --comments`.
- **Search for duplicates**: `glab issue list --all --search "<terms>" -O json`.
- **Comment on an issue**: `glab issue note <number> --message "..."`.
- **Merge requests**: GitLab calls PRs "merge requests". Use `glab mr create`, `glab mr view`, `glab mr note`, etc.

Infer the repo from `git remote -v`; `glab` does this automatically when run inside a clone.

## Linking issues

An MR description closes an issue with `Closes #<n>`.
