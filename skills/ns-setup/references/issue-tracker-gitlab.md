# Issue tracker: GitLab

Issues and specs for this repo live as GitLab issues. Use the [`glab`](https://gitlab.com/gitlab-org/cli) CLI for all operations.

## Conventions

- **Create an issue**: `glab issue create --title "..." --description "..."`. Use a heredoc for multi-line descriptions.
- **Read an issue**: `glab issue view <number> --comments`. Use `-F json` for machine-readable output.
- **List issues**: `glab issue list -O json` with appropriate `--label` filters.
- **Search for duplicates**: `glab issue list --all --search "<terms>" -O json`.
- **Comment on an issue**: `glab issue note <number> --message "..."`. GitLab calls comments "notes".
- **Apply / remove labels**: `glab issue update <number> --label "..."` / `--unlabel "..."`. Multiple labels can be comma-separated or by repeating the flag.
- **Close**: `glab issue close <number>`. It takes no closing comment, so post the explanation first with `glab issue note <number> --message "..."`, then close.
- **Merge requests**: GitLab calls PRs "merge requests". Use `glab mr create`, `glab mr view`, `glab mr note`, etc., the same shape as `gh pr ...` with `mr` in place of `pr` and `note`/`--message` in place of `comment`/`--body`.

Infer the repo from `git remote -v`; `glab` does this automatically when run inside a clone.

## Team

Project members (Developer role and above) are the team. Everyone else is external.

External comments: wait

`wait` (default): a skill drafts any comment addressed to an external person and stops for a human. `allow`: skills post such comments on this tracker themselves, with the AI disclaimer. Set `allow` only if unattended runs may talk to reporters.

## Merge requests as a triage surface

**MRs as a request surface: no.** _(Set to `yes` if this repo treats external merge requests as feature requests; `ns-triage` reads this flag.)_

When set to `yes`, MRs run through the same labels and states as issues, using the `glab mr` equivalents:

- **Read an MR**: `glab mr view <number> --comments` and `glab mr diff <number>` for the diff.
- **List external MRs for triage**: `glab mr list -F json`, then keep only MRs whose author is not a project member or owner.
- **Comment / label / close**: `glab mr note`, `glab mr update --label`/`--unlabel`, `glab mr close`.

GitLab numbers issues and MRs separately, so `#42` is unambiguous once you know which surface the maintainer means.

## Linking issues

- **Child of a parent**: put `Part of #<parent>` at the top of the child's description. (On tiers with epics, an epic may hold the parent instead.)
- **Blocking**: GitLab's native blocking link, added with the `/blocked_by #<n>` quick action posted as a note: `glab issue note <child> --message "/blocked_by #<blocker>"`. It is a Premium/Ultimate feature. On the free tier, put a `Blocked by: #<n>, #<n>` line at the top of the description.
- **Is it unblocked?** Read the links with `glab api projects/:id/issues/<child-iid>/links` and the `Blocked by` line. It is unblocked when every blocker is closed.

## When a skill says "publish to the issue tracker"

Create a GitLab issue.

## When a skill says "fetch the relevant ticket"

Run `glab issue view <number> --comments`.
