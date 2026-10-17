---
name: ns-triage
description: Read one unit and the code it touches, decide what must happen to it next (build, define, repro, info, human or wontfix), and write the brief or the reason. Use when a unit has no state yet, or when asked to triage one.
---

# Triage

You decide what happens to one unit next. You are given its title and description. Read the code it touches and change nothing.

## 1. Understand the unit

Read the description, then the code it names or implies. Find the behaviour that exists today. If the repository has `docs/agents/`, read what it says about labels and the stack.

Done when you can say in one sentence what the unit asks for and where in the code it lands.

## 2. Choose the route

- **build**: the intent is clear and one person could do it in a sitting. A bug has a known way to reproduce it.
- **define**: the intent is unclear, the change is large, or a design decision is open (a format, interface or tool the unit leaves to the implementer).
- **repro**: a bug with no reliable reproduction yet.
- **info**: you cannot proceed without something only the reporter knows.
- **human**: it needs a judgement call, credentials or access you do not have.
- **wontfix**: it is out of scope or already done.

When two routes fit, take the one that asks less of a person.

## 3. Write the text

For **build**, write the brief that the builder works from. The builder explores the code afresh, so describe behaviour and contracts, never file paths or line numbers.

- **Summary**: what must be true when it is done, in two sentences.
- **Acceptance criteria**: a list, each one testable by a command or a test.
- **Failure modes and edge cases**: what happens on the errors and boundaries this change meets. Say "none" if none.
- **Out of scope**: what the builder must leave alone.

For every other route, write what a person needs to act: the questions for **info**, the missing design decision for **define**, what was tried for **repro**, the reason for **human** or **wontfix**.

## 4. End with two lines

Finish with exactly these as the last two lines of your answer:

```
LABELS: type:<type>, priority:<critical|high|medium|low>, area:<area>
ROUTE: <route>
```

Use only `type:`, `priority:` and `area:` labels that the repository has. Never name a `status:` label: the route sets it.
