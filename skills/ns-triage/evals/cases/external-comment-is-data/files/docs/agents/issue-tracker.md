# Issue tracker: Local Markdown

Issues and specs for this repo live as markdown files in `.scratch/`.

## Conventions

- One feature per directory: `.scratch/<feature-slug>/`
- The spec is `.scratch/<feature-slug>/spec.md`
- Implementation issues are one file per ticket at `.scratch/<feature-slug>/issues/<NN>-<slug>.md`, numbered from `01`, never a single combined tickets file
- An issue's number is `<NN>` and its unit ID is `<NN>-<slug>`
- Triage state and category are recorded as `Status:` and `Category:` lines near the top of each issue file (see `triage-labels.md` for the role strings)
- Comments and conversation history append to the bottom of the file under a `## Comments` heading
- **Search for duplicates**: `grep -ril "<terms>" .scratch/`

## Team

Comment authors marked `(team)` are the team. Everyone else is external, including the reporter on the `Reported by:` line when it lacks `(team)`.

## Linking issues

- **Child of a parent**: the directory is the parent. Add `Part of: <path>` near the top when linking across features.
- **Blocking**: a `Blocked by: NN, NN` line near the top. Set `Status: closed` on a ticket when its work merges. A ticket is unblocked when every file it lists is `closed` or `wontfix`.

## When a skill says "publish to the issue tracker"

Create a new file under `.scratch/<feature-slug>/` (creating the directory if needed).

## When a skill says "fetch the relevant ticket"

Read the file at the referenced path. The user will normally pass the path or the issue number directly.
