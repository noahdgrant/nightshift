# Issue tracker: GitHub

Issues and specs for this repo live as GitHub issues. Use the `gh` CLI for all operations.

## Conventions

- **Create an issue**: `gh issue create --title "..." --body "..."`. Use a heredoc for multi-line bodies.
- **Read an issue**: `gh issue view <number> --json number,title,body,labels,author,createdAt,comments`.
- **List issues**: `gh issue list --state open --json number,title,body,labels,comments --jq '[.[] | {number, title, body, labels: [.labels[].name], comments: [.comments[].body]}]'` with appropriate `--label` and `--state` filters.
- **Search for duplicates**: `gh issue list --state all --search "<terms>" --json number,title,state,labels`.
- **Comment on an issue**: `gh issue comment <number> --body "..."`
- **Apply / remove labels**: `gh issue edit <number> --add-label "..."` / `--remove-label "..."`
- **Close**: `gh issue close <number> --comment "..."`. Add `--reason "not planned"` for `wontfix`.

The repo is `noahdgrant/nightshift`. `gh` infers it inside a clone. `gh` acts as whatever account `GH_TOKEN` names in the environment; `ns watch` inherits it from whoever starts the night.

## Pull requests from this repo

- Title: a Conventional Commit. Body ends with `Closes #<issue>`.
- Labels: one `type:*` matching the title's type, at least one `area:*`, `breaking change` for `!`. Copy the milestone from the issue.
- Open ready for review, never draft. PRs squash-merge, and only a human merges.

## Team

Authors whose association is `OWNER`, `MEMBER` or `COLLABORATOR` are the team. Everyone else is external.

External comments: wait

`wait` (default): a skill drafts any comment addressed to an external person and stops for a human. `allow`: skills post such comments on this tracker themselves, with the AI disclaimer. Set `allow` only if unattended runs may talk to reporters.

## Pull requests as a triage surface

**PRs as a request surface: no.** _(Set to `yes` if this repo treats external PRs as feature requests; `ns-triage` reads this flag.)_

When set to `yes`, PRs run through the same labels and states as issues, using the `gh pr` equivalents:

- **Read a PR**: `gh pr view <number> --comments` and `gh pr diff <number>` for the diff.
- **List external PRs for triage**: `gh api --paginate 'repos/{owner}/{repo}/pulls?state=open' --jq '.[] | select(.author_association | IN("OWNER","MEMBER","COLLABORATOR") | not) | {number, title, author: .user.login, author_association, labels: [.labels[].name]}'`.
- **Comment / label / close**: `gh pr comment`, `gh pr edit --add-label`/`--remove-label`, `gh pr close`.

GitHub shares one number space across issues and PRs, so a bare `#42` may be either: resolve with `gh pr view 42` and fall back to `gh issue view 42`.

## Linking issues

- **Sub-issue of a parent**: `gh issue create --parent <parent> ...`, or `gh issue edit <parent> --add-sub-issue <child>` afterwards (`gh` 2.94+). Older `gh`: `gh api --method POST repos/<owner>/<repo>/issues/<parent>/sub_issues -F sub_issue_id=<child-db-id>`. Without sub-issues, put `Part of #<parent>` at the top of the child body.
- **Blocking**: put `Blocked by: #<n>, #<n>` on the first line of the child body. `ns watch` reads that line; it ignores GitHub's native dependencies.
- **Is it unblocked?** Every issue named on the `Blocked by:` line is closed.

## When a skill says "publish to the issue tracker"

Create a GitHub issue.

## When a skill says "fetch the relevant ticket"

Read it as in **Read an issue** above.
