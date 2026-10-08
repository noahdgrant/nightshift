---
name: ns-swarm
description: Fan out parallel fresh-context workers, drain them, and return one PASS/ISSUES/BLOCKED report. Use when work splits into independent slices, when racing several attempts at one brief, or when another skill asks for a fan-out.
metadata:
  upstream: cursor/plugins@ccb5507cec15:pstack/skills/swarm
---

# Swarm

Fan out N workers. They cover separate slices or race the same brief. The parent waits, aggregates, and returns one report.

A **worker** is a fresh-context agent. It sees only its brief and the repo, never the parent's chat.

## 1. Frame

1. State the **done predicate** and the report the swarm must return.
2. Choose the **shape**:
   - **Partition**: each worker gets one slice (a reviewer axis, a feature, a file group). Coverage is complete only when every slice has a result.
   - **Race**: N workers get identical briefs. Declare the selection rule before spawning: `first pass` (take the first PASS), `rank all` (order every result by evidence), or `best-of` (pick one, say why).
   - A mix of both is fine. Name which workers are slices and which are race arms.
3. Set N from the caller or derive it from the shape. N is total workers, not a concurrency limit.
4. Name each worker's **role** for `ns ask` (the caller supplies it, e.g. `review.security`, `verify`). Roles map to a harness and model in the user's config. This skill never names a model.
5. Give every worker that writes its own worktree (see "Writers" below). Read-only workers share the parent's worktree.
6. When workers verify or measure commits, the brief names the exact SHAs. A measurement brief also names the method: sample count, what one sample is, order.

Done when the shape, N, roles, selection rule (for a race) and any SHAs are written down.

## 2. Write the briefs

Every brief stands alone. A reader with no chat history can act on it. Each one holds:

- the goal and the done predicate
- the exact slice or race arm, and what is out of scope
- the input files to read, by path
- how to verify
- the report format below

Write each brief to a file: `<artifacts>/swarm/<run>/<worker>.brief.md`, where `<artifacts>` is the caller's `.ns/<unit-id>/` or a temp dir when there is no unit. Files keep the brief out of shell quoting, and the parent can audit what each worker saw.

### Worker report format

```markdown
VERDICT: PASS | ISSUES | BLOCKED
SCOPE: <slice or arm>
SHAS: <commits checked, when the brief named any>
METHOD: <how it was checked>

- <file:line> <one-line issue> | evidence: <command + output, or quoted code>
```

- `PASS`: the slice meets the done predicate, with the evidence that shows it.
- `ISSUES`: lists every issue the worker can prove, not only the first, each with evidence.
- `BLOCKED`: names what the worker could not get (hardware, credentials, a missing file).

## 3. Fan out

Pick the first launch mode the harness supports, and record which one ran:

1. **Subagents.** Spawn all N in one message, in the background. Pass each its brief file.
2. **`ns ask`.** Run one headless process per worker, all at once. Read-only workers use the default mode. Workers that edit files or run commands (verify, build) add `--write --cwd <their worktree>`:
   ```bash
   # workers.tsv: one "<worker> <role>" pair per line
   while read -r w role; do
     { ns ask --role "$role" --prompt-file "$RUN/$w.brief.md" \
         > "$RUN/$w.result.md" 2> "$RUN/$w.err"; echo $? > "$RUN/$w.exit"; } &
   done < "$RUN/workers.tsv"
   wait
   ```
   In `<worker>.exit`, 3 means the role is not configured, 4 means its harness is missing. 5 means the harness has no write mode configured. Retry that worker under the caller's fallback role if it named one, otherwise run it inline.
3. **Inline.** No subagents and no `ns`: run each brief yourself, one at a time, clearing your working notes between briefs. Write "ran inline, sequentially, not in fresh contexts" at the top of the report.

If a worker drops out, carry on with the rest and record the dropout.

### Writers

A worker that edits files gets its own worktree and branch, so writers never share a working tree:

```bash
ns worktree new <unit-id>--<worker> --base ns/<unit-id>
# without ns:
git worktree add -b ns/<unit-id>--<worker> ../<repo>.worktrees/<unit-id>--<worker> ns/<unit-id>
```

The brief names that path. The parent merges or cherry-picks the winning or accepted branches afterwards, then removes the worktrees with `ns worktree remove` or `git worktree remove`.

## 4. Aggregate

Read every result file or subagent reply.

- A result with no `VERDICT` line, or one missing the SHAs and method its brief named, is malformed. Respawn that worker once. After a second miss, record a **gap**.
- A missing result is a gap. A gap never counts as a pass.
- Partition: the swarm passes only when every slice reports `PASS`.
- Race: apply the selection rule declared in step 1, unchanged.
- Merge duplicate issues across workers and note which workers raised each one.

Done when every worker has a result, a dropout note, or a gap entry.

## 5. Report

Return one report. Summarise worker output in one-line issues and keep raw dumps in the result files, linked by path.

```markdown
Launch mode: subagents | ns ask | inline (sequential)
Shape: partition | race (rule: first pass | rank all | best-of) | mixed

| Worker | Role | Scope | Verdict | Issues |
|---|---|---|---|---|

Issues
- <file:line> <one line> (workers: a, c) | evidence: <short>

Gaps and dropouts
- <worker>: <why>

Overall: PASS | ISSUES | BLOCKED
```

Overall is `PASS` only when every required slice passed and there are no gaps. Any gap or `BLOCKED` slice makes it `BLOCKED`; any proven issue makes it `ISSUES`.
