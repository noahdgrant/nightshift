# Labels

What the repository's labels mean. Read by `ns-triage`.

- **`area:`**: where a unit lives. `area:cli` is the `ns` Rust CLI, `area:skills` the `ns-*` skills, `area:evals` the skill evals, and `area:repo` repo-wide docs, conventions and config. A unit gets the area it changes most.
- **`type:`**: the kind of change, named as the Conventional Commit types are (`type:feat`, `type:fix`, `type:refactor`, `type:docs`, `type:test`, `type:chore` and so on). It becomes the type in the change request's title.
- **`priority:`**: `priority:critical` is fix next, `priority:high` blocks other work or breaks the factory or is a security issue, `priority:medium` is normal, `priority:low` can wait.
- **`status:`**: where the unit is. `ns` owns it while a unit runs, so do not set it by hand on a unit in progress.
  - `status:needs-triage`: new, not yet triaged.
  - `status:needs-info`: waiting on the reporter.
  - `status:needs-repro`: a bug with no reliable failure yet; goes to `ns-troubleshoot`.
  - `status:needs-define`: intent unclear or the change is large; goes to `ns define`.
  - `status:ready-for-agent`: has a spec; `ns auto` takes it.
  - `status:in-progress`: `ns run` is working on it.
  - `status:waiting-for-human`: a person has to act and the unit is not stuck: a change request waiting for a merge, or work that needs hardware, credentials or a decision.
  - `status:stuck`: a state failed every attempt, or the verification could not prove the change; a person looks first and resumes it with `ns run <unit>`.
  - `status:split`: split into child units; it closes when they are done.
  - `status:wontfix`: closed as not planned.
- **`needs-human-merge`**: on a change request `ns` left for a person to merge, because it touches a `[merge] human_review` path.
- **`breaking change`**: the change alters behavior users or skills depend on.
- **`autorelease:`**: release-please's own labels; leave them alone.
