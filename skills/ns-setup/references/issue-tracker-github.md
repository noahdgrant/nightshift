# Issue tracker: GitHub

Issues and specs for this repo live as GitHub issues. Use the `gh` CLI for all operations.

## Conventions

- **Disclosure**: every issue body, comment and PR body an agent posts opens with a 🤖 line naming the agent, then a blank line: `🤖 *Posted by nightshift, an AI agent.*` from nightshift (`ns-contract`, Disclosure). Titles stay plain.

Infer the repo from `git remote -v`; `gh` does this automatically when run inside a clone.

## Operations

Skills name an operation by its heading and run the command beneath it (`ns-contract`, Tracker operations).

### Create an issue

`gh issue create --title "..." --body "..."`. Use a heredoc for multi-line bodies.

### Read an issue

`gh issue view <number> --json number,title,body,labels,author,createdAt,comments`.

### List issues

`gh issue list --state open --json number,title,body,labels,comments --jq '[.[] | {number, title, body, labels: [.labels[].name], comments: [.comments[].body]}]'` with appropriate `--label` and `--state` filters.

### Search for duplicates

`gh issue list --state all --search "<terms>" --json number,title,state,labels`.

### Comment on an issue

`gh issue comment <number> --body "..."`

### Apply or remove a label

`gh issue edit <number> --add-label "..."` / `--remove-label "..."`

### Close an issue

`gh issue close <number> --comment "..."`. Add `--reason "not planned"` for `wontfix`.

### Read an author's association

`gh issue view <n> --json author,body,comments` gives every comment's `authorAssociation`, and `gh api repos/{owner}/{repo}/issues/<n> --jq .author_association` gives the issue's own.

### List labels

`gh label list`

### Create a label

`gh label create "<name>" --description "..."`

### Link a child to its parent

`gh issue create --parent <parent> ...`, or `gh issue edit <parent> --add-sub-issue <child>` right after filing (`gh` 2.94+). Older `gh`: `gh api --method POST repos/<owner>/<repo>/issues/<parent>/sub_issues -F sub_issue_id=<child-db-id>`. Without sub-issues, put `Part of #<parent>` at the top of the child body.

### List a parent's children

`gh api repos/<owner>/<repo>/issues/<parent>/sub_issues`

### Add a blocking link

GitHub's native issue dependencies: `gh api --method POST repos/<owner>/<repo>/issues/<child>/dependencies/blocked_by -F issue_id=<blocker-db-id>`, where `<blocker-db-id>` is the blocker's numeric **database id** (`gh api repos/<owner>/<repo>/issues/<n> --jq .id`, _not_ the `#number` or `node_id`). Where dependencies aren't available, put a `Blocked by: #<n>, #<n>` line at the top of the child body, right after the 🤖 line.

### Check an issue is unblocked

Count open blockers: `gh api repos/<owner>/<repo>/issues/<n>/dependencies/blocked_by --jq '[.[] | select(.state == "open")] | length'`. Zero means unblocked. `issue_dependencies_summary.blocked_by` keeps counting a blocker after it closes, so it can't gate.

### Open a PR

`gh pr create --base "$base" --head "<branch>" --title "<title>" --body-file "$body"`

### Read the new PR's URL and draft state

`gh pr view --json url,isDraft`, run on the PR's branch.

### Mark a PR ready

`gh pr ready <pr>`

### Read a PR's state

`gh pr view <pr> --json state`. `MERGED` means it merged.

### Find a branch's PR

`gh pr list --head <branch> --state all --json url,state`

### Read a PR's merge state

`gh pr view <pr> --json mergeable,mergeStateStatus,isDraft,reviewDecision,statusCheckRollup,headRefOid`

### Read a PR's checks

`gh pr checks <pr>`

### Wait for a PR's checks

`gh pr checks <pr> --watch`

### Read a PR's review threads

```bash
gh api graphql -f query='query($o:String!,$r:String!,$n:Int!){repository(owner:$o,name:$r){pullRequest(number:$n){reviewThreads(first:100){nodes{id isResolved isOutdated comments(first:20){nodes{databaseId author{login} body path line}}}}}}}' -F o=<owner> -F r=<repo> -F n=<pr>
```

### Reply to a review thread

With the reply in `payload.json` as `{"body": "..."}`:

```bash
gh api --method POST repos/<owner>/<repo>/pulls/<pr>/comments/<comment-id>/replies --input payload.json
```

### Resolve a review thread

```bash
gh api graphql -f query='mutation($id:ID!){resolveReviewThread(input:{threadId:$id}){thread{isResolved}}}' -F id=<thread-id>
```

### Re-run CI

`gh run rerun <run-id>`

### Read a PR's merge commit

`gh pr view <pr> --json mergeCommit,baseRefName`

### List a commit's CI runs

`gh run list --branch "<branch>" --commit <sha> --json conclusion,status`. A `conclusion` of `failure` is red.

### Read a PR and its diff

`gh pr view <pr> --comments` and `gh pr diff <pr>` for the diff.

### List external PRs

`gh api --paginate 'repos/{owner}/{repo}/pulls?state=open' --jq '.[] | select(.author_association | IN("OWNER","MEMBER","COLLABORATOR") | not) | {number, title, author: .user.login, author_association, labels: [.labels[].name]}'`.

### Comment on, label or close a PR

`gh pr comment`, `gh pr edit --add-label`/`--remove-label`, `gh pr close`.

## Team

Authors whose association is `OWNER`, `MEMBER` or `COLLABORATOR` are the team. Everyone else is external.

External comments: wait

`wait` (default): a skill drafts any comment addressed to an external person and stops for a human. `allow`: skills post such comments on this tracker themselves. Set `allow` only if unattended runs may talk to reporters.

## Pull requests as a triage surface

**PRs as a request surface: no.** _(Set to `yes` if this repo treats external PRs as feature requests; `ns-triage` reads this flag.)_

When set to `yes`, PRs run through the same labels and states as issues, with the PR operations above: **Read a PR and its diff**, **List external PRs**, and **Comment on, label or close a PR**.

GitHub shares one number space across issues and PRs, so a bare `#42` may be either: resolve with `gh pr view 42` and fall back to `gh issue view 42`.

## Audit

audit issues per run: 5

The most issues one `ns-agent-readiness` run files. With no such line, it files 5.

## Linking issues

- **Sub-issue of a parent**: **Link a child to its parent** and **List a parent's children** above.
- **Blocking**: **Add a blocking link** above.
- **Is it unblocked?** **Check an issue is unblocked** above.

## When a skill says "publish to the issue tracker"

Create a GitHub issue.

## When a skill says "fetch the relevant ticket"

Read it as in **Read an issue** above.
