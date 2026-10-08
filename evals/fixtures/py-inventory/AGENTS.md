# AGENTS.md

`inventory` is a small pure-Python package (src layout) with a CLI. Domain terms are defined in `GLOSSARY.md`; use them in code, tests and issues.

Every time-dependent call takes `now` explicitly. Keep it that way: no `datetime.now()` outside the CLI.

## Agent skills

Repo settings the `ns-*` skills read. Edit the files directly to change them.

- **Stack**: Python 3.11+; `python3 -m pytest -q`. See `docs/agents/stack.md`.
- **Verify**: not set up yet. See `docs/agents/verify.md`.
- **Issue tracker**: local markdown under `.scratch/`. See `docs/agents/issue-tracker.md`.
- **Triage labels**: default. See `docs/agents/triage-labels.md`.
- **Domain docs**: single-context. See `docs/agents/domain.md`.
