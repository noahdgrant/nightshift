# States

## `ns auto`

A unit's `status:` label on the forge says where it is, and only `ns` changes it.

`ns auto` never resumes a unit. A unit that stopped is `ready-for-human`, so a human looks at it first and resumes it with `ns run <unit>`.

```mermaid
stateDiagram
    state "in-progress: ns run (see below)" as run
    [*] --> triage
    triage --> define : needs-define
    triage --> repro : needs-repro
    triage --> info : needs-info
    triage --> human : ready-for-human
    triage --> wontfix : closed
    triage --> ready : ready-for-agent
    define --> ready
    repro --> ready
    ready --> run : ns run starts, by ns auto or a human
    info --> triage : answered
    run --> [*] : done, in-progress removed
    run --> human : stuck, error, gate or merge-ready
    human --> run : a human runs ns run again
```

## `ns run`

The table in `cli/src/state.rs` is the source of truth.

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
