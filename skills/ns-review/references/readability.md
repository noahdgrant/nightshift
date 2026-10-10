# Readability and simplicity reviewer

What must hold on this axis is the **Readability** section of the [quality bar](../../ns-contract/references/quality-bar.md). This file is how to find where it doesn't.

Can another engineer, or agent, understand this code without the author explaining it?

## Look for

- **Names** that reveal what a function, variable or type does or holds, consistent with the repo's conventions. `tmp`, `data`, `result`, `handle` without context are findings.
- **Control flow** that reads straight down: no nested conditional expressions, deep nesting, or flag arguments steering a function two ways.
- **Size**: could this be done in fewer lines? 1000 lines where 100 suffice is a failure.
- **Clever tricks** that should be plain code.
- **Abstractions earning their cost**: no generalising before the third use, no pass-through wrappers.
- **Dead code**: unused variables, parameters, imports, back-compat shims, `# removed` markers, commented-out code.
- **Bolted-on conditionals**: a new `if` in an unrelated flow is a design smell, not a nit. Push it into its own helper, state or policy.
- **Repeated conditionals on the same shape** signal a missing model or dispatcher.

## Standards

Check the diff against every standards file in the brief. Cite the file and rule for each violation. Skip anything the repo's linter or formatter already enforces.

On top of the repo's standards, apply this smell baseline. Each smell is a judgement call ("possible Speculative Generality"), never a hard violation, and a documented repo standard overrides it.

- **Mysterious Name**: the name doesn't say what it does. Rename; if no honest name comes, the design is murky.
- **Duplicated Code**: the same logic shape in more than one hunk or file. Extract it and call it from both.
- **Speculative Generality**: parameters, hooks or abstraction for needs the contract doesn't have. Delete and inline.
- **Middle Man**: a function or class that mostly delegates onward. Call the real target.

## Remedies

Name the move, not just the problem: rename, inline, delete the wrapper, collapse duplicate branches, replace a conditional chain with a lookup table or typed model, extract a helper.

Comments are the comments reviewer's axis. Leave them to it.
