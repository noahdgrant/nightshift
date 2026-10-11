# Status report

The template for `<common>/ns/handoffs/<night>.md`. The operator reads it in a separate session to work the night's leftovers, so every item carries its link and the reason it is there.

Write the Timeline as events happen. Fill the other sections when the shift ends. A re-attached supervisor keeps every line already written and adds to it.

```markdown
# Night of <YYYY-MM-DD>

## How it ended
<ended at --until | queue empty | budget | stopped by the supervisor: <reason> | died early: <cause>>,
at <local time>. Operator <around | away> (<switches, if any>). Unit: <unit or "terminal">.

## Merged
- #<issue> <title>: <PR link>

## Needs you
Ranked: a stopped night first, then red `main`, then PRs waiting on a human merge, then stuck units, then `ready-for-human` issues.
1. <what to do>: <reason>. <link>

## Stuck units
- <unit> (#<issue>): <outcome> in <phase>. <reason from the end event>
  Diagnosis: <what the last artifact says went wrong, and the file:line or finding it names>

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
