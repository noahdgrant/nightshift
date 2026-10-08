---
name: sf-writing-for-humans
description: Cut AI tells and write plain prose people read. Use when writing or editing docs, READMEs, PR and issue text, commit bodies, emails, or messages.
metadata:
  upstream:
    - cursor/plugins@ccb5507cec15:pstack/skills/unslop
    - cursor/plugins@ccb5507cec15:pstack/skills/technical-writing
---

# Writing for humans

Write so a tired engineer understands it on the first read. For documents an agent reads (skills, `AGENTS.md`), load `sf-writing-for-agents` instead.

## Process

1. Write the real names. The codebase is the word list: the actual symbol, file, flag, command, or glossary term, never a synonym or a description of it. Call each thing by one name everywhere.
2. Put the point first. The reader learns what to do or know in the first sentence, and the condition comes before the instruction it guards ("To delete the unit, run...").
3. Scan for the patterns below and rewrite. Preserve meaning and match the intended tone.
4. Add voice (next section). Removing patterns is half the job. Sterile prose is just as obvious.
5. Self-audit. Ask "what makes this obviously AI generated?" and fix what remains.

Done means every pattern below was checked against the text, and every sentence survives the test in rule 27.

When a rule makes a sentence worse, fix the sentence another way or leave it alone. The rules serve the reader.

## Adding voice

- **Have a view.** Where the text weighs trade-offs, say what you make of them instead of listing pros and cons. Reference text stays dry.
- **Vary rhythm.** Short sentences land a point. Longer ones carry a fact with its condition or consequence. Split a sentence that carries two thoughts, and keep a long one that carries one.
- **Be specific.** Not "schema changes can cause issues" but "a column rename fails the build".
- **Use "I" or "we" when it fits.** First person is not unprofessional.

## Patterns to detect and fix

Rule numbers are stable ids that other skills can cite. A removed rule leaves a gap.

### Content

1. **Puffery.** "pivotal moment", "testament to", "evolving landscape", "setting the stage for", "deeply rooted". State what happened.
2. **Name-dropping.** A list of sources with no context. Pick one and say what it said.
3. **Superficial -ing phrases.** "highlighting...", "ensuring...", "reflecting...", "showcasing...". Delete, or expand into a real claim.
4. **Promotional language.** "groundbreaking", "seamless", "powerful", "robust", "stunning". Use neutral descriptions.
5. **Vague attributions.** "Experts believe", "Industry reports suggest". Name the source or delete.
6. **Formulaic challenges.** "Despite challenges, X continues to thrive." Replace with specific facts.

### Language

7. **AI vocabulary.** Additionally, crucial, delve, enduring, enhance, fostering, garner, interplay, intricate, landscape (abstract), pivotal, showcase, tapestry (abstract), testament, underscore, vibrant. Use plain words.
8. **Fancy ways to say "is".** "serves as", "stands as", "boasts", "features". Write "is" or "has".
9. **"Not just X, but Y."** State the point directly.
10. **Rule of three.** Ideas forced into groups of three. Use the natural number.
11. **Synonym cycling.** Four names for one thing in one paragraph. Pick one and repeat it.
12. **False ranges.** "from X to Y" where X and Y are not on a scale. List the items.

### Style

13. **Em dashes.** Use none. Use a period or a comma. Parentheses, en dashes, and hyphen-as-dash only trade one tell for another. If a thought needs separation, end the sentence.
14. **Colon overuse.** Colons belong before a list or an example, not as mid-sentence connectors. "If you're coming from cron: instead of times, you describe conditions" becomes "Describe when the job runs in plain conditions."
15. **Boldface overuse.** Bold is for a rare key term, not every proper noun.
16. **Inline-header lists.** The tell is a bold label and colon that restates the line ("**Performance:** Performance improved..."). Convert to prose. A bold lead-in ending in a period, followed by new detail, is fine.
17. **Title case headings.** Use sentence case. A heading carries the point, not just the topic.
18. **Decorative emojis.** Remove them from headings and bullets.
19. **Curly quotes.** Use straight quotes.

### Communication artifacts

20. **Chatbot phrases.** "I hope this helps!", "Let me know if...", "Certainly!", "Found the smoking gun!" Remove.
21. **Cutoff disclaimers.** "While specific details are limited..." Find the facts or cut the sentence.
22. **Sycophancy.** "Great question! You're absolutely right!" Respond directly.

### Filler

23. **Filler phrases.** "In order to" becomes "to". "Due to the fact that" becomes "because". "It is important to note that" gets deleted.
24. **Excessive hedging.** "could potentially be argued that it might" becomes "may".
25. **Generic conclusions.** "The future looks bright." State the plan or the fact.

### Jargon

26. **Abstract metaphor nouns.** Substrate, wedge, vector, locus, nexus, primitive (as noun), harness (as metaphor), surface (as in "API surface"), bedrock, scaffolding (as metaphor), paradigm, gold-plating, ratchet (as metaphor), evacuate (for moving code), endgame, north star, flywheel. Pick the concrete word: "base", "add", "way", "more than the job needs", "a limit that only tightens", "move out", "the last phase".

### Plain speech

27. **Say what it does, not how it feels.** "types that follow your schema" names a feeling. "A column rename fails the build" names the mechanism. Ask what the sentence tells the reader to do or know, and write that. If you can't restate it as an instruction, fact, or number, cut it. If it could appear unchanged in another project's docs, it says nothing about this one.
28. **Dense sentences.** If the reader has to backtrack, split the sentence or drop clauses. Split instructions over about 20 words.
29. **Passive voice.** Name the actor: "queries are validated" becomes "the compiler validates queries". Passive is fine when the actor is unknown or beside the point.
30. **Adverbs propping up weak verbs.** "runs quickly" becomes the number. "significantly improves" becomes the measured delta.
31. **Fancy words.** "utilize" and "leverage" become "use", "facilitate" becomes "help", "in the event that" becomes "if".
32. **Mannered prose.** Aphorisms ("wire it or delete it"), rhetorical fragments, personified code ("the plan holds it"), figurative verbs ("rides along"). Say it literally.
33. **Over-compression.** Dropped articles, verbless fragments, arrows, and abbreviations make the reader decode. "Parser rejects bad date → exit 2, no write" becomes "The parser rejects a bad date, exits with code 2, and writes nothing."
34. **Two readings.** Keep "only" and "not" next to the word they change. Make every "it", "this", and "they" point at one obvious noun, and repeat the noun when in doubt. Break long noun strings ("the budget check script config" becomes "the config for the script that checks the budget"). Use "a, b, or both", not "a/b" or "and/or".

## PR text, issues, and commit bodies

- A PR body or issue is a briefing. A reader with the diff learns why, what is left out, what could break, and how it was proved, in under a minute. Link logs, metric tables, and long evidence instead of pasting them.
- A commit body says why. It does not restate the subject line.
- Make every count, path, and command true at the commit that lands it.

## Worked example

Before:

> Configuration of the retry budget parameters is performed via budget.toml. It's important to note that running with --write, which updates the committed budget to reflect the current count, should only be done when lowering it. If exceeded, CI fails.

After:

> `budget.py` reads the retry budget from `budget.toml`. If the count goes over the budget, CI fails. Run `budget.py --write` only to lower the budget.
