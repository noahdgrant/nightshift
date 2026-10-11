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

- Baseline, in `$V/repo`, with `.nightshift/nightshift.toml` holding `name = "demo"`.

- **GitHub.** Run `cap forge-github "$NS" doctor` and read `.forge.github`. `configured` and `token_resolved` are `true`, `account` is `octocat`, and `grep -c fake-token $OUT/forge-github.out` is `0`.
- **GitLab.** Copy `$NS_CONFIG` to `$V/c-gl.toml`, append `[forge.gitlab]\ntoken_env = "MY_GL"\nhost = "gitlab.example.com"`, and run `cap forge-gitlab env NS_CONFIG=$V/c-gl.toml MY_GL=gl-fake "$NS" doctor`, reading `.forge.gitlab`. `configured` and `token_resolved` are `true`.
- **One source.** Write `[forge.github]\ntoken_command = "x"\ntoken_env = "Y"` to `$V/c-forge.toml` and run `cap forge-one-source env NS_CONFIG=$V/c-forge.toml "$NS" doctor`. `config.parses` is `false` and `config.error` says `set exactly one of token_command and token_env`.
- **GitLab queue.** Set `[queue]\nsource = "gitlab"` in the factory file (restore `name = "demo"` alone afterwards). Seed `$V/ghd/gl-issues.jsonl` with `{"iid":9,"title":"Issue 9","labels":["status:ready-for-agent"],"description":"","author":{"id":1,"username":"u1"}}`, `labels-9` with `status:ready-for-agent`, and `gl-members.json` with `[{"id":1,"username":"u1","access_level":40}]`. Run `cap forge-source-gitlab env NS_CONFIG=$V/c-gl.toml MY_GL=gl-fake "$NS" watch --dry-run --no-self-update`. `queue` holds number `9`, and `$V/ghd/glab-calls` shows `api --paginate projects/:id/issues?...`.

## Gotchas

- The fake `gh` has no `api user` case; the `hook.sh` Launch writes adds it. Without it `account` is `null`.
- A set `GH_TOKEN` or `GITLAB_TOKEN` wins over the config. `env.sh` unsets both.
- On GitLab only the queue, labels and comments go through `glab`; `ns run`'s merge step still uses `gh`.
