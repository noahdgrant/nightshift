# Events

What `ns run`, `ns auto` and `ns log --json` print on stdout, for a session or a script that reads it. Each event is one JSON object on one line. What `ns` writes to stderr is not described here and is not a contract.

A test (`cli/tests/cli/events.rs`) runs the commands through the fakes and compares what they print with the tables below. It fails when `ns` prints an event or a field that is not listed, when a listed one is never printed, and when a field has a type that is not listed.

## The contract

- A field is added and never renamed or removed. A reader ignores fields it does not know.
- A break (a field renamed or removed, a type changed, an event dropped) is deliberate, and the commit says so: its title or body names the event and the field.
- There is no version field on an event. The version of `ns` is the version of the contract.
- The types are JSON types. A field is present when its row says nothing else; one that is printed only sometimes says when.
- `unit` is a string on the events of a run's states and a number on `blocked_by`, on `stopped` and on the events of `ns auto`. That is how it was first printed, and it stays.

The `squash`, `read`, `notified` and `notify` entries that `ns run` and `ns auto` append to the run log (`.git/ns/runs.jsonl`) are not printed, so they are not events here. `notified` and `notify` are described under Notify in the README.

## `ns run`

`ns run <unit>` prints one event for each thing it does, in order, as it does it. A reader tells the events apart by the field they have: `blocked_by`, `took_over`, `waiting`, `not_merged`, `stopped`, `session`, `heartbeat`, `from`, or else `state`. `ns auto` prints the events of the units it runs in the same way, among its own.

### `ns run: state`

Printed when a run begins at a state, when it resumes there, and when it finishes: the last line of a run that reached `done`.

| Field | Type | Meaning |
|---|---|---|
| `unit` | `string` | The unit's number. |
| `state` | `string` | The state the run is in: `setup`, `build`, `verify`, `review`, `ship`, `retrospective`, `clean_up` or `done`. |
| `refresh` | `string` | Only on `done`, for a unit whose change request is open and whose run is not from `ns auto`: `current` when the change request was up to date with the base branch, `refreshed` when `ns` rebased it and ran the checks again. |

### `ns run: session start`

Printed when a harness session starts.

| Field | Type | Meaning |
|---|---|---|
| `unit` | `string` | The unit's number. |
| `session` | `string` | Always `start`. |
| `state` | `string` | The state the session runs for. |
| `n` | `number` | The number the session's transcript is kept under, from 1. |
| `model` | `string or null` | The model the session uses; `null` is the harness's own. |

### `ns run: heartbeat`

Printed every 60 seconds while a session runs (`--heartbeat-seconds` sets the interval, `0` turns it off).

| Field | Type | Meaning |
|---|---|---|
| `unit` | `string` | The unit's number. |
| `heartbeat` | `string` | The state the session runs for. |
| `seconds` | `number` | The seconds the session has run. |
| `last_tool` | `string or null` | The name of the last tool the harness ran; `null` before it has run one. |

### `ns run: session end`

Printed when a session ends, however it ends: finished, timed out, a usage limit or a harness that failed.

| Field | Type | Meaning |
|---|---|---|
| `unit` | `string` | The unit's number. |
| `session` | `string` | Always `end`. |
| `state` | `string` | The state the session ran for. |
| `n` | `number` | The number the session's transcript is kept under. |
| `seconds` | `number` | The seconds the session ran. |
| `tokens` | `object` | `input`, `output`, `cache_read` and `cache_write`, each a number or `null` when the harness's stream does not report that kind. Never summed across kinds. |

### `ns run: move`

Printed when a unit moves from one state to another, and also written to the run log. A move that goes on to the next state has no `attempt` and no `reason`.

| Field | Type | Meaning |
|---|---|---|
| `unit` | `string` | The unit's number. |
| `from` | `string` | The state the unit leaves, or `stuck`. |
| `to` | `string` | The state the unit moves to: the next one, an earlier one to try again, or `stuck`. |
| `to_attempt` | `number` | Which attempt of `to` this will be, from 1. Not on the move that sends a finished unit back to `build` after a refresh failed. |
| `attempt` | `number` | Which attempt of `from` just ended. Only on a move that is not to the next state. |
| `reason` | `string` | Why the move is not to the next state: `failed`, `went_wrong`, `inconclusive`, `base_moved`, or a sentence when a stuck unit goes back to `build` because nothing changed since the check failed. Only on a move that is not to the next state. |
| `model` | `string or null` | The model the session of `from` used; `null` is the harness's own. Only on a move out of `build`, `verify` or `review`. |
| `verdict` | `string` | `PASS`, `FAIL` or `INCONCLUSIVE`, as the state judged. Only on a move out of `verify` or `review`, and on the `waiting` event after it. Left out when the evidence has none of these three, for example `ERROR`. |
| `findings` | `number` | The findings the state counted, summed over review sessions. On every move out of `verify` or `review`, with or without `verdict`. |
| `artifact` | `string` | The path in the workspace of the notes the state kept: `verify-<n>.md`, `review-<n>.md` or `build.md`. Only on a move out of `build`, `verify` or `review`. |
| `sessions` | `array` | One `{"name":..,"verdict":..}` for each review session; a session's `verdict` is left out when its evidence has none of the three, for example `ERROR`. Only on a move out of `review`. |
| `evidence` | `string` | The text of `evidence.md`, why the refresh failed. Only on the move from `done` back to `build` that a failed refresh causes. |

### `ns run: waiting`

Printed when the run stops before a state because gates ask it to (the default), and the next `ns run` goes on from there.

| Field | Type | Meaning |
|---|---|---|
| `unit` | `string` | The unit's number. |
| `waiting` | `string` | The state the unit waits to run. |
| `verdict` | `string` | Repeated from the move just before, when it had it. |
| `findings` | `number` | Repeated from the move just before, when it had it. |
| `artifact` | `string` | Repeated from the move just before, when it had it. |
| `sessions` | `array` | Repeated from the move just before, when it had it. |

### `ns run: took_over`

Printed when `ns run` finds that a change request for the unit's branch exists, opened by a person, and takes it over. Also written to the run log.

| Field | Type | Meaning |
|---|---|---|
| `unit` | `string` | The unit's number. |
| `took_over` | `number` | The number of the change request. |
| `merged_at` | `number` | Only when the change request has merged: when, in seconds since the epoch. |

### `ns run: blocked_by`

Printed, and nothing else runs, when open units block the unit and `--ignore-blockers` is not given. The exit status is not zero.

| Field | Type | Meaning |
|---|---|---|
| `unit` | `number` | The unit's number. |
| `blocked_by` | `array` | The numbers of the open units that block it. |

### `ns run: not_merged`

Printed when `ship` does not merge the change request, so a person does: the change falls under `[merge] human_review`, or the base branch kept moving. Also written to the run log, except when the base branch kept moving.

| Field | Type | Meaning |
|---|---|---|
| `unit` | `string` | The unit's number. |
| `state` | `string` | Always `ship`. |
| `not_merged` | `string` | Why, in a sentence. |

### `ns run: stopped`

Printed when the harness has reached its usage limit. The unit is left as it was, with its status as before, and the exit status is not zero.

| Field | Type | Meaning |
|---|---|---|
| `unit` | `number` | The unit's number. |
| `stopped` | `string` | Always `usage_limit`. |
| `reason` | `string` | The limit, in a sentence, with when it resets when the harness says. |

## `ns auto`

`ns auto` prints the events of each unit it runs, as `ns run` does, and the events below.

### `ns auto: plan`

Printed by `ns auto --dry-run` instead of running anything: what a night would do. Each list holds objects with `unit` (a number) and `title`; `skipped` holds `unit` and `why`.

| Field | Type | Meaning |
|---|---|---|
| `queue` | `array` | The units that would be run, in order. |
| `triage` | `array` | The units that would be triaged first. |
| `repro` | `array` | The units that would be given a reproduction first. |
| `skipped` | `array` | The units left out, each with `why`. |

### `ns auto: stopped`

Printed when the night ends before the units run out. `usage_limit` and `until` end it with a zero exit status; `went_wrong` with a non-zero one.

| Field | Type | Meaning |
|---|---|---|
| `stopped` | `string` | Why: `usage_limit` when the harness's limit resets after the time `--until` gives or is not known, `went_wrong` when units in a row went wrong in the harness, `until` when the time given to `--until` has come. |
| `reason` | `string` | Only with `usage_limit` and `went_wrong`: the cause, in a sentence. |
| `units` | `array` | Only with `went_wrong`: the numbers of the units that went wrong, in order. |

### `ns auto: unit`

Printed after each unit `ns auto` ran, whether it finished or not.

| Field | Type | Meaning |
|---|---|---|
| `unit` | `number` | The unit's number. |
| `ok` | `boolean` | Whether the run ended without an error. |
| `status` | `array` | The unit's `status:` labels on the forge when it ended. |
| `change_request` | `string or null` | The URL of the unit's change request; `null` when it has none. |
| `tokens` | `object` | The night's running total after this unit: `input`, `output`, `cache_read` and `cache_write`, kept apart and never summed, over the sessions kept so far in the night, `known` and `unknown` (sessions whose tokens cannot be read, left out of the totals). With no known session the four totals are absent. |
| `error` | `string or null` | What went wrong; `null` when `ok`. |

## `ns log --json`

`ns log <unit> --json` prints each message of the unit's harness sessions, oldest session first, as one line, instead of the rendered text. `--state` and `--last` pick the same sessions as without `--json`. A message is what the agent said, a tool it called or what the tool returned.

### `ns log --json: message`

| Field | Type | Meaning |
|---|---|---|
| `state` | `string` | The state the session ran for. |
| `n` | `number` | Which session of that state, from 1. |
| `kind` | `string` | `text` for what the agent said, `tool_call` for a tool it called, `tool_result` for what the tool returned. |
| `text` | `string` | Only on `text` and `tool_result`: the whole text. |
| `tool` | `string` | Only on `tool_call`: the name of the tool. |
| `input` | `object` | Only on `tool_call`: the tool's input as the harness gave it. |
