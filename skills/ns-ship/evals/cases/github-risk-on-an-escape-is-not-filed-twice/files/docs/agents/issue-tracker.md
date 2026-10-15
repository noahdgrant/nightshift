# Issue tracker: GitHub

Issues and PRs for this repo live on GitHub. Use the `gh` CLI for all operations.

Infer the repo from `git remote -v`; `gh` does this automatically when run inside a clone.

## Operations

### Create an issue

`gh issue create --title "..." --body "..."`.

### Read an issue

`gh issue view <number>`

### Comment on an issue

`gh issue comment <number> --body "..."`.

### Open a PR

`gh pr create --base "$base" --head "<branch>" --title "<title>" --body-file "$body"`

### Read the new PR's URL and draft state

`gh pr view --json url,isDraft`, run on the PR's branch.

### Mark a PR ready

`gh pr ready <pr>`

### Read a PR's state

`gh pr view <pr> --json state`. `MERGED` means it merged.

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

## Linking issues

A PR body closes an issue with `Closes #<n>`.
