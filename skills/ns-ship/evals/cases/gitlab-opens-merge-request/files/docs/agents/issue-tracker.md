# Issue tracker: GitLab

Issues and merge requests for this repo live on GitLab. Use the `glab` CLI for all operations. GitLab calls PRs "merge requests" (`<iid>`).

Infer the repo from `git remote -v`; `glab` does this automatically when run inside a clone.

## Operations

A PR in a heading is a merge request here.

### Create an issue

`glab issue create --title "..." --description "..."`.

### Read an issue

`glab issue view <number> --comments`.

### Search for duplicates

`glab issue list --all --search "<terms>" -O json`.

### Comment on an issue

`glab issue note <number> --message "..."`.

### Open a PR

`glab mr create --source-branch "<branch>" --target-branch "$base" --title "<title>" --description-file "$body" --yes`

### Read the new PR's URL and draft state

`glab mr view -F json`, run on the merge request's branch: `web_url`, `iid`, `draft`.

### Mark a PR ready

`glab mr update <iid> --ready`

### Read a PR's state

`glab mr view <iid> -F json`. A `state` of `merged` means it merged.

### Read a PR's merge state

`glab mr view <iid> -F json`: `detailed_merge_status`, `has_conflicts`, `draft`, `sha`, `head_pipeline`. Merge-ready is `detailed_merge_status: mergeable`, a `head_pipeline` with `status: success` at the `sha` you pushed, and no discussion with a resolvable note left unresolved.

### Read a PR's checks

`glab ci status --branch <branch> -F json`

### Wait for a PR's checks

`glab ci status --branch <branch> --wait`

### Read a PR's review threads

`glab api --paginate projects/:id/merge_requests/<iid>/discussions`: each discussion's `id`, `notes[].resolvable` and `notes[].resolved`. `:id` is the current project, which `glab` fills in.

## Linking issues

An MR description closes an issue with `Closes #<n>`.
