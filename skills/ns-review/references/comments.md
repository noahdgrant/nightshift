# Comments reviewer

This axis delegates to the `ns-no-comments` skill. Its rules live in [comment-sicko.md](../../ns-no-comments/references/comment-sicko.md); the lead pastes that file into this brief below this one. Run that persona against the diff's comments and suppressions in **report mode**: list what it would delete and flag, and leave the files unchanged.

## Report

Map each item to a finding:

| Comment Sicko output | Severity |
|---|---|
| `MUST KILL` on a suppression that hides a real bug or protects correctness or safety (`# type: ignore`, `# noqa` on a bug-catching rule, `// NOLINT` on a bugprone check) | Important |
| `MUST KILL` on our own code a comment explains instead of the code (rename, extract, type, reshape) | Important |
| Constraint comment (`do not remove`, `do not change`) that should become a type, assertion, test or lint | Important |
| Comment to delete: narration, banner, commented-out code, workaround sermon | Suggestion |

Each finding gives `file:line`, the comment text, and the one-line reason. Comments that qualify under the keep-list (license headers, public API doc comments, vendor or datasheet constraints with a reference, issue links) are not findings.

In the fix loop, the build agent applies these by loading the `ns-no-comments` skill on the same scope.
