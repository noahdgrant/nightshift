# States

The table in `cli/src/state.rs` is the source of truth for `ns run`.

## `ns auto`

Where a unit goes before it is run. Triage decides, and the unit's label on the forge records it. Only a ready unit goes to `ns run`; `define`, `repro` and the waiting states are not built yet.

```mermaid
stateDiagram
    state "ns run (see below)" as run
    [*] --> triage
    triage --> define : needs-define
    triage --> repro : needs-repro
    triage --> info : needs-info
    triage --> human : ready-for-human
    triage --> wontfix : closed
    triage --> run : ready-for-agent
    define --> run
    repro --> run
    info --> triage : answered
    run --> [*]
```

## `ns run`

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
