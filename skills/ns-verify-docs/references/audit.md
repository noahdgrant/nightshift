# Audit mode

An audit covers every current doc, which is too much for one context to read in full and compare. Fan steps 4 and 5 out by topic, so each worker reads a cluster of docs that share facts, then bring the findings back and run steps 6 and 7.

## 1. Run the checks

Step 2 of the skill, for the whole repo. Fix what they find first, so the workers compare clean docs.

## 2. Split by topic

Give each worker a topic, not a fixed file list: files come and go, and a contradiction lives between the docs that share a topic. Take the topics from the system's own parts, as the glossary and the requirements keys in `docs/agents/docs.md` name them. A typical split:

- **The data path**: what moves between the parts, with its timings, sizes, retries and states.
- **The user-facing product**: setup, accounts, permissions, notifications, privacy.
- **Hardware and its tests**, when there is any: boards, power, radios, sensors, test rigs.
- **Tooling, delivery and process**: the CLI, admin tools, CI, deploys, migrations, access, the coding standards.

Add a topic for any part of the system none of them covers.

## 3. Brief each worker

Load the `ns-swarm` skill, one worker per topic. Each worker is a **fresh-context agent** working read-only. Its brief gives:

- its topic, and that it finds its own docs from `docs/agents/docs.md` and by searching;
- to read the glossary and every doc in its topic in full;
- [contradictions.md](contradictions.md), with every kind to be applied;
- the skill's step 5, so each finding comes back confirmed or unsure, with both passages quoted at `path:line`;
- the decisions already recorded that it must hold docs to, when the audit follows a round of fixes.

It reports confirmed findings, unsure findings, and the kinds it found clean.

## 4. Merge and verify

Merge the reports: the same finding from two topics is one finding, and two workers agreeing on it raises confidence. Re-read both passages yourself for every finding that would change behaviour, privacy or security before it goes in the report. Then return to the skill's step 6.
