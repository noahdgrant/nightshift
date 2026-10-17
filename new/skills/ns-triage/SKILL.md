---
name: ns-triage
description: Read one unit and the code it touches, decide what must happen to it next (build, define, repro, info, human or wontfix), and give the reason. Use when a unit has no state yet, or when asked to triage one.
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

## 3. Say why

Give the reason for the route in a few sentences, for the person who reads it later. Say what the unit asks for, and for any route but **build** what is missing. Do not write the brief or any other text for the unit: you decide the route and nothing else.

## 4. End with two lines

Finish with exactly these as the last two lines of your answer:

```
LABELS: type:<type>, priority:<critical|high|medium|low>, area:<area>
ROUTE: <route>
```

Use only `type:`, `priority:` and `area:` labels that the repository has. Never name a `status:` label: the route sets it.
