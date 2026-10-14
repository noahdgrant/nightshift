# Forge config

The forge settings decide where `ns run` and `ns watch` get forge tokens and which forge `ns watch` reads. `[forge.github]` and `[forge.gitlab]` in the ns config each name exactly one of `token_command` or `token_env`, plus an optional `host`. `[queue] source` in the factory definition picks `github` or `gitlab`. `ns doctor` reports each forge without printing the token.

## Sub-features

- `forge-github` resolves the GitHub token and reports the account `gh api user` returns.
- `forge-gitlab` resolves a GitLab token from `token_env`.
- `forge-one-source` rejects a forge with both or neither token key.
- `forge-source-gitlab` sends `ns watch`'s queue through `glab`.

## How to get to it (user POV)

- `[forge.github]` / `[forge.gitlab]` in `$NS_CONFIG`, read by `ns doctor`, `ns run` and `ns watch`.
- `[queue] source` in `.nightshift/nightshift.toml`, read by `ns watch` and `ns factory validate`.

## Driving it with ns

Preconditions:

- Baseline, with the factory file:

  ```bash
  "$V/sandbox" sh -c 'mkdir -p .nightshift && echo "$1" > .nightshift/nightshift.toml' _ 'name = "demo"'
  ```

- **GitHub.** Read `.forge.github`: `configured` and `token_resolved` are `true` and `account` is `octocat`. `forge-github-no-token` prints `0`: the token never appears in the report.

  ```bash
  "$V/sandbox" cap forge-github ns doctor
  "$V/sandbox" cap forge-github-no-token sh -c 'grep -c fake-token "$OUT/forge-github.out"'
  ```

- **GitLab.** Copy the config, add `[forge.gitlab]` with a `token_env`, and read `.forge.gitlab`: `configured` and `token_resolved` are `true`.

  ```bash
  "$V/sandbox" sh -c 'cp "$NS_CONFIG" "$V/c-gl.toml" && printf "$1" >> "$V/c-gl.toml"' _ '[forge.gitlab]\ntoken_env = "MY_GL"\nhost = "gitlab.example.com"\n'
  "$V/sandbox" cap forge-gitlab env NS_CONFIG="$V/c-gl.toml" MY_GL=gl-fake ns doctor
  ```

- **One source.** `config.parses` is `false` and `config.error` says `set exactly one of token_command and token_env`.

  ```bash
  "$V/sandbox" sh -c 'printf "$1" > "$V/c-forge.toml"' _ '[forge.github]\ntoken_command = "x"\ntoken_env = "Y"\n'
  "$V/sandbox" cap forge-one-source env NS_CONFIG="$V/c-forge.toml" ns doctor
  ```

- **GitLab queue.** Set `[queue] source = "gitlab"` in the factory file and seed GitLab issue 9. `queue` holds number `9`, and `forge-source-gitlab-calls` shows `api --paginate projects/:id/issues?...`. The last line restores the factory file.

  ```bash
  "$V/sandbox" sh -c 'printf "$1" > .nightshift/nightshift.toml' _ 'name = "demo"\n[queue]\nsource = "gitlab"\n'
  "$V/sandbox" sh -c 'echo "$1" >> "$V/ghd/gl-issues.jsonl"' _ '{"iid":9,"title":"Issue 9","labels":["status:ready-for-agent"],"description":"","author":{"id":1,"username":"u1"}}'
  "$V/sandbox" sh -c 'echo status:ready-for-agent > "$V/ghd/labels-9"'
  "$V/sandbox" sh -c 'echo "$1" > "$V/ghd/gl-members.json"' _ '[{"id":1,"username":"u1","access_level":40}]'
  "$V/sandbox" cap forge-source-gitlab env NS_CONFIG="$V/c-gl.toml" MY_GL=gl-fake ns watch --dry-run --no-self-update
  "$V/sandbox" cap forge-source-gitlab-calls cat "$V/ghd/glab-calls"
  "$V/sandbox" sh -c 'echo "$1" > .nightshift/nightshift.toml' _ 'name = "demo"'
  ```

## Gotchas

- The fake `gh` has no `api user` case; the `hook.sh` that `sandbox.sh --new` writes adds it. Without it `account` is `null`.
- A set `GH_TOKEN` or `GITLAB_TOKEN` wins over the config. The sandbox unsets both; pass one with `env` on the one command that needs it.
- On GitLab only the queue, labels and comments go through `glab`; `ns run`'s merge step still uses `gh`.
