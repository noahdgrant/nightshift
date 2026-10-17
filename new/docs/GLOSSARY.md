# Nightshift

Nightshift takes a unit of work to a merged change with coding agents. This is the shared language of the code, the docs and the skills of the rebuild. The names are neutral on purpose: no tool-specific word (GitHub's "pull request", git's "worktree") outside the module that talks to that tool.

## Language

**Unit**:
One piece of work, tracked on the forge and named by its number there. A unit owns one workspace and one branch, and moves through its lifecycle until its change merges.
_Avoid_: Job, task, run

**State**:
A step of a unit's lifecycle: `setup`, `build`, `verify`, `review`, `ship`, `clean_up`, then `done` or `stuck`. `ns run` moves a unit from one state to the next.
_Avoid_: Phase, stage

**Attempt**:
One go at a state. A failed state is tried again until its attempts run out.
_Avoid_: Retry, try

**Stuck**:
The end state of a unit whose state failed every attempt. It waits for a human.
_Avoid_: Failed, blocked

**Workspace**:
A directory where one unit's work happens, kept apart from the main checkout and from other units' workspaces.
_Avoid_: Worktree, sandbox, checkout

**Branch**:
The line of changes a unit's workspace is on, named `ns/<unit id>`.

**Brief**:
The text a unit's `build` works from: what to change and how to know it is done. Written during triage from the unit's description on the forge.
_Avoid_: Spec, prompt

**Change request**:
A proposal on the forge to merge a unit's branch into the main one, reviewed and checked before it merges.
_Avoid_: Pull request, merge request, PR, MR

**Artifact**:
A file a state writes into the unit's workspace under `.ns/<unit id>/`, for the next state to read.
_Avoid_: Output, report

## Tools behind a seam

Each is a trait with one implementation chosen in one place. The rest of the code uses the trait's words.

**Harness**:
A coding agent that does a state's work from a prompt, headless. Claude Code is the first.
_Avoid_: Agent, model, LLM

**Forge**:
The service that hosts the units and change requests. GitHub is the first, GitLab the next.
_Avoid_: Tracker, host, platform

**VCS**:
The version control that holds the code, branches and workspaces. Git is the first.
_Avoid_: SCM, repo tool

**Language pack**:
The lint rules and checks for one programming language. Rust is the first.
_Avoid_: Plugin, ruleset

**Runner**:
What starts `ns` unattended, for example a systemd user unit. Not built yet.
_Avoid_: Scheduler, daemon
