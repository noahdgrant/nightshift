# Coding standards

These hold in any language. A repository adds its own, for its language and its tools, in a document that points here.

## A change request closes a unit

The body of a change request ends with `Closes #<unit>`, naming a unit that exists. The tracker's number belongs in the change request, never in a committed file (next section).

## No issue numbers in committed files

A committed file never names an issue or change request by number. The number means nothing to a reader who can't see the tracker, and it goes stale when the work moves forge. Say what the issue was about.

Bad:

```
// Workaround for #730.
```

Good:

```
// A phase can leave read-only directories, which make the workspace unremovable.
```

## Single source of truth

Keep each meaning in a single place and point to it instead of restating it. A copy can drift from the original, and then the two disagree with no way to tell which is right.

Bad: the list of states in the enum and again in the glossary.

Good: the glossary says "the list is in `docs/STATES.md`".

## Retry what can pass, where it is safe

A call to a service outside the program (a forge, a remote, a harness) fails for reasons that pass on their own, such as a dropped connection, a 5xx or a rate limit. Repeat such a call a few times, waiting longer each time, and say each failure on stderr. Do it in the one place that makes the call, so every caller gets it, and make the wait settable so a test can set it to zero.

Repeat only a call that is safe to make twice, and only a failure known to be transient. A permanent error (a 404, no permission, a missing program) fails at once, and so does an error nobody has classified: repeating it hides it and costs time. A call that is not safe to repeat, such as opening or merging a change request, is made once; the step that made it runs again and looks before it acts.

Bad: a loop around every call that retries whatever went wrong.

Good: a list of the failures that can pass, a retry in the place that makes the call, and one test per kind of failure.
