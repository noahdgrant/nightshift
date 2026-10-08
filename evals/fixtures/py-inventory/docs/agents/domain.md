# Domain Docs

How the `ns-*` skills consume this repo's domain documentation when exploring the codebase.

## Before exploring, read these

- **`GLOSSARY.md`** at the repo root, or
- **`GLOSSARY-MAP.md`** at the repo root if it exists: it points at one `GLOSSARY.md` per context. Read each one relevant to the topic.
- **`docs/adr/`**: read ADRs that touch the area you're about to work in. In multi-context repos, also check `<context>/docs/adr/` for context-scoped decisions.

If any of these files don't exist, **proceed silently**. Leave their absence unflagged, and leave creating them to `ns-domain-modeling` (reached through `ns-triage` and `ns-define`), which writes them lazily when terms or decisions actually get resolved.

## This repo

Single-context. The glossary is `GLOSSARY.md` at the repo root. There is no `docs/adr/` yet.

## Use the glossary's vocabulary

When your output names a domain concept (in an issue title, a refactor proposal, a hypothesis, a test name), use the term as defined in `GLOSSARY.md`. Stick to it rather than drifting to synonyms the glossary lists under `_Avoid_`.

If the concept you need isn't in the glossary yet, that's a signal: either you're inventing language the project doesn't use (reconsider) or there's a real gap (note it for `ns-domain-modeling`).

## Flag ADR conflicts

If your output contradicts an existing ADR, surface it explicitly rather than silently overriding:

> _Contradicts ADR-0007 (event-sourced orders), but worth reopening because…_
