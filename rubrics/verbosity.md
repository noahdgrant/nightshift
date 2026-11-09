---
name: verbosity
pass: 0.6
---

Judge whether the build summary and the comments the change adds are as long as a reviewer needs and no longer.

Look for:

- a build summary that retells the diff line by line, repeats itself, or pads a short change;
- comments that say what the code plainly says, restate a name, or narrate the change instead of explaining why;
- long comments or doc comments where one line would do.

Do not count a note a reviewer needs: an assumption, something noticed, a reason that is not in the code, or a person's decision the summary must name.

Scale:

- 1.0: the summary and every comment say what a reviewer needs and stop.
- 0.8: a little padding, such as one comment that restates its code.
- 0.6: noticeable padding, but the key points are easy to find.
- 0.4: the reviewer must dig through padding to find what matters.
- 0.2: most of the text is padding.
- 0.0: the text is all padding, or buries what the reviewer needs.
