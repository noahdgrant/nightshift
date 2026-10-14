# `ns artifact`

## `ns artifact check <file> [--phase <phase>]`

Validates a phase artifact's frontmatter against the `ns-artifact/2` schema, the one [`skills/ns-contract/SKILL.md`](../../skills/ns-contract/SKILL.md) (Units of work and artifacts) defines. `--phase` names the phase that wrote it (`triage`, `build`, `verify`, `review`, `review-fix`, `ship`), and the artifact's `phase` must match. Each phase skill runs it on its own artifact before it finishes.

It checks:

- the frontmatter is the first thing in the file, closes with `---`, and parses as YAML
- `schema: ns-artifact/2` is present; a file with no `schema` key is an `ns-artifact/1` artifact and fails
- `unit`, `phase`, `status`, `cause` and `updated` are present, and `phase`, `status`, `cause` and `next` hold one of their values
- `status: pass` has `cause: none`; `fail` and `blocked` have a cause other than `none`, a `next` other than `none`, and a `reason`; `split` has a `reason` and is triage's only
- `reason` is one line of at most 200 characters
- a passed triage names `next`; every phase but triage has a `sha`; a passed ship has a `pr`
- `sha` and `delta_of` are commit shas, `updated` is a UTC timestamp, `findings` and `touches` are lists of strings
- `findings` holds finding ids (`I3`), only on review and review-fix artifacts, and is non-empty under `cause: findings_open`

Unknown keys are ignored. Every error is one line naming the key and what to write. Prints `{ok, file, schema, phase, status, cause, next, errors}`; when the artifact is invalid, the values are as written, or `null` when absent. Exits 0 when valid, 1 when invalid, and 2 on a usage error or a file it can't read.

An `ns-artifact/1` artifact is read, not checked, by mapping the old routing onto `cause` and `next`: a pass routes to the next phase, a blocked artifact to a human (`cause: denied` when verify's first body line is `denied by`), a first body line starting `base moved` to build, and a failed ship to the phase its body names first (a human when it names neither verify nor review), a failed triage to a human, and any other fail to build.
