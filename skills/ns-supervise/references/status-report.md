# Status report

The template for `<common>/ns/handoffs/nightshift-<night>.md`. The operator reads it in a separate session to work the night's leftovers, so every item carries its link and the reason it is there.

Write the Timeline as events happen. Fill the other sections when the shift ends. A re-attached supervisor keeps every line already written and adds to it.

```markdown
# nightshift-<YYYY-MM-DD>

## How it ended
<ended at --until | queue empty | budget | main_red_unfixed: fix issue #<n> ended <outcome> | stopped by the supervisor: <reason> | died early: <cause>>,
at <local time>. Operator <around | away> (<switches, if any>). Unit: <unit or "terminal">.

## Merged
- #<issue> <title>: <PR link>

## Needs you
Ranked: a stopped night first, then red `main`, then PRs waiting on a human merge, then stuck units, then `ready-for-human` issues.
1. <what to do>: <reason>. <link>

## Stuck units
- <unit> (#<issue>): <outcome> in <phase>. <reason from the end event>
  Diagnosis: <what the last artifact says went wrong, and the file:line or finding it names>

## Split parents
Issues triage split into child issues. Each waits under `status:split` until `ns watch` closes it once its children are done, so none is a stuck unit, and only one sent to `ready-for-human` also goes under `Needs you`. The run log has them: an `end` event with outcome `split` starts the wait, and a `split_parent` event records each change (`action` `close`, `ready_for_human` or `relabel`, with `children` and, while some are open, `open`).
- #<issue> <title>: <open> of <children> children open (<#child, ...>)
- #<issue> <title>: <closed, every child done | sent to ready-for-human, #<child> not planned> at <local HH:MM>

## Quality
`ns quality --since <night start, UTC>`: first-pass yield, findings per 100 lines, leftovers.
<#338's metrics too, once `ns quality` prints them.>

## Anomalies
- <anything new: a failure kind not seen before, a long silence, an OOM kill, a restart>

## Tomorrow's queue
`ns status --json` `.next`: <the ready issues in ns watch's order, or "queue unavailable">

## Timeline
- <local HH:MM> <started | attached | re-attached> (<around | away>)
- <local HH:MM> ping [<key>]: <the ping line>
- <local HH:MM> stopped: <reason>
```

Write `none` under a section with nothing in it.
