# States

## Before `ns run`

`ns watch` triages a unit, and its route sets the unit's state label on the forge. Only `build` leads to `ns run`. `define`, `repro` and the waiting routes are not built yet.

```mermaid
stateDiagram
    [*] --> triage
    triage --> build : ready-for-agent
    triage --> define : needs-define
    triage --> repro : needs-repro
    triage --> info : needs-info
    triage --> human : ready-for-human
    triage --> wontfix : closed
    define --> build
    repro --> build
    build --> run
```

## `ns run`

How a unit moves through `ns run`. The table in `cli/src/state.rs` is the source of truth.

```mermaid
stateDiagram
    [*] --> setup
    setup --> build
    build --> verify
    verify --> review
    review --> ship
    ship --> clean_up
    clean_up --> done
    done --> [*]

    verify --> build : build is wrong
    review --> build : findings
    ship --> build : checks failed
    setup --> setup : state went wrong
    build --> build : state went wrong
    verify --> verify : check went wrong
    review --> review : state went wrong
    ship --> ship : state went wrong
    clean_up --> clean_up : state went wrong

    setup --> stuck : out of attempts
    build --> stuck : out of attempts
    verify --> stuck : out of attempts
    review --> stuck : out of attempts
    ship --> stuck : out of attempts
    clean_up --> stuck : out of attempts
    stuck --> [*]
```
