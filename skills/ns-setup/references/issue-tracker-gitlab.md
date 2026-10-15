# Issue tracker: GitLab

Issues and specs for this repo live as GitLab issues. Use the [`glab`](https://gitlab.com/gitlab-org/cli) CLI for all operations.

## Conventions

- **Disclosure**: every issue body, comment and merge request description an agent posts opens with a 🤖 line naming the agent, then a blank line: `🤖 *Posted by nightshift, an AI agent.*` from nightshift (`ns-contract`, Disclosure). Titles stay plain.
- **Merge requests**: GitLab calls PRs "merge requests" (`<iid>`), comments "notes", and a review thread a discussion. `:id` in a `glab api` path is the current project, which `glab` fills in.

Infer the repo from `git remote -v`; `glab` does this automatically when run inside a clone.

## Operations

Skills name an operation by its heading and run the command beneath it (`ns-contract`, Tracker operations). A PR in a heading is a merge request here.

### Create an issue

`glab issue create --title "..." --description "..."`. Use a heredoc for multi-line descriptions.

### Read an issue

`glab issue view <number> --comments`. Use `-F json` for machine-readable output.

### List issues

`glab issue list -O json` with appropriate `--label` filters.

### Search for duplicates

`glab issue list --all --search "<terms>" -O json`.

### Comment on an issue

`glab issue note <number> --message "..."`.

### Apply or remove a label

`glab issue update <number> --label "..."` / `--unlabel "..."`. Multiple labels can be comma-separated or by repeating the flag.

### Close an issue

`glab issue close <number>`. It takes no closing comment, so post the explanation first with `glab issue note <number> --message "..."`, then close.

### Read an author's association

`glab issue view <n> -F json` gives the issue author's `id`, and `glab api projects/:id/issues/<n>/notes` each note's `author.id`. `glab api projects/:id/members/all/<user-id>` gives that user's `access_level`; 30 (Developer) and above is the team, and a not-found answer means not a member.

### List labels

`glab label list`

### Create a label

`glab label create --name "<name>" --description "..."`

### Link a child to its parent

Put `Part of #<parent>` at the top of the child's description. (On tiers with epics, an epic may hold the parent instead.)

### List a parent's children

`glab issue list --all --search "Part of #<parent>" -O json`, keeping only issues whose description links the parent as a whole token.

### Add a blocking link

GitLab's native blocking link, added with the `/blocked_by #<n>` quick action posted as a note: `glab issue note <child> --message "/blocked_by #<blocker>"`. It is a Premium/Ultimate feature. On the free tier, put a `Blocked by: #<n>, #<n>` line at the top of the description, right after the 🤖 line.

### Check an issue is unblocked

Read the links with `glab api projects/:id/issues/<child-iid>/links` and the `Blocked by` line. It is unblocked when every blocker is closed.

### Open a PR

`glab mr create --source-branch "<branch>" --target-branch "$base" --title "<title>" --description-file "$body" --yes`

### Read the new PR's URL and draft state

`glab mr view -F json`, run on the merge request's branch: `web_url`, `iid`, `draft`.

### Mark a PR ready

`glab mr update <iid> --ready`

### Read a PR's state

`glab mr view <iid> -F json`. A `state` of `merged` means it merged.

### Find a branch's PR

`glab mr list --source-branch <branch> --all -F json`: `web_url`, `state`.

### Read a PR's merge state

`glab mr view <iid> -F json`: `detailed_merge_status`, `has_conflicts`, `draft`, `sha`, `head_pipeline`. Merge-ready is `detailed_merge_status: mergeable`, a `head_pipeline` with `status: success` at the `sha` you pushed, and no discussion with a resolvable note left unresolved. `not_approved` is a required approval: a wait, not a blocker.

### Read a PR's checks

`glab ci status --branch <branch> -F json`

### Wait for a PR's checks

`glab ci status --branch <branch> --wait`

### Read a PR's review threads

`glab api --paginate projects/:id/merge_requests/<iid>/discussions`: each discussion's `id`, `notes[].resolvable` and `notes[].resolved`.

### Reply to a review thread

With the reply in `payload.json` as `{"body": "..."}`:

```bash
glab api --method POST projects/:id/merge_requests/<iid>/discussions/<discussion-id>/notes -H 'Content-Type: application/json' --input payload.json
```

### Resolve a review thread

`glab api --method PUT projects/:id/merge_requests/<iid>/discussions/<discussion-id> -f resolved=true`

### Re-run CI

`glab api --method POST projects/:id/pipelines/<pipeline-id>/retry`

### Read a PR's merge commit

`glab mr view <iid> -F json`: `merge_commit_sha`, `target_branch`.

### List a commit's CI runs

`glab api "projects/:id/pipelines?sha=<sha>&ref=<branch>"`. A `status` of `failed` is red.

### Read a PR and its diff

`glab mr view <iid> --comments` and `glab mr diff <iid>` for the diff.

### List external PRs

`glab mr list -F json`, then keep only MRs whose author is not a project member or owner.

### Comment on, label or close a PR

`glab mr note`, `glab mr update --label`/`--unlabel`, `glab mr close`.

## Team

Project members (Developer role and above) are the team. Everyone else is external.

External comments: wait

`wait` (default): a skill drafts any comment addressed to an external person and stops for a human. `allow`: skills post such comments on this tracker themselves. Set `allow` only if unattended runs may talk to reporters.

## Merge requests as a triage surface

**MRs as a request surface: no.** _(Set to `yes` if this repo treats external merge requests as feature requests; `ns-triage` reads this flag.)_

When set to `yes`, MRs run through the same labels and states as issues, with the PR operations above: **Read a PR and its diff**, **List external PRs**, and **Comment on, label or close a PR**.

GitLab numbers issues and MRs separately, so `#42` is unambiguous once you know which surface the maintainer means.

## Audit

audit issues per run: 5

The most issues one `ns-agent-readiness` run files. With no such line, it files 5.

## Linking issues

- **Child of a parent**: **Link a child to its parent** and **List a parent's children** above.
- **Blocking**: **Add a blocking link** above.
- **Is it unblocked?** **Check an issue is unblocked** above.

## When a skill says "publish to the issue tracker"

Create a GitLab issue.

## When a skill says "fetch the relevant ticket"

Read it as in **Read an issue** above.
