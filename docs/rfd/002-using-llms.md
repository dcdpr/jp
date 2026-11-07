# RFD 002: Using LLMs in the JP Project

- **Status**: Implemented
- **Category**: Process
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-17
- **Summary**: Contributors may use LLMs freely as tools, and remain responsible
  for the quality of everything they submit.

## Summary

The JP project encourages using LLMs for reading, research, editing, debugging,
code review, and code generation.
Contributors remain responsible for everything they submit (code, prose,
reviews, and comments), regardless of how it was produced.

## Core Principle

**You own what you ship.** Every commit, comment, review, and message carries
its author's name as a statement of responsibility.
If you use an LLM to produce an artifact, that artifact is yours: you reviewed
it, you stand behind it, and you will maintain it.
"An LLM wrote it" is never an excuse.

The same standard applies to any tool or source, whether a compiler, a linter, a
code generator, a Stack Overflow answer, or a colleague's suggestion.
The tool helps; you decide.

## Values

Five values guide how we use LLMs, in priority order:

- **Responsibility**: LLMs are tools directed by humans.
  Human judgment stays in the loop: even when an LLM generates code, tests,
  documentation, or prose, the contributor who submits the output is responsible
  for it.

- **Rigor**: Used carefully, LLMs sharpen our thinking by exposing gaps in
  reasoning and suggesting alternatives.
  Used carelessly, they replace clear thinking with generated noise.
  Use LLMs only where they strengthen rigor.

- **Empathy**: Every piece of writing, code, and commit message has a human
  reader on the other end.
  Respect that reader's time and attention when using LLMs.

- **Teamwork**: LLM use must not erode trust within the team.
  Using an LLM to debug a tricky issue is natural, whereas submitting unreviewed
  LLM output shifts your work onto the reviewer.
  What matters is taking responsibility for the work, not disclosing the tool.

- **Urgency**: LLMs can speed up work, but velocity must never come at the
  expense of responsibility, rigor, empathy, or teamwork, or sacrifice direction
  and quality.

## Uses of LLMs

### LLMs as Readers

LLMs excel at reading comprehension.
They can quickly summarize documents, answer questions about a specification, or
explain an unfamiliar codebase.

Using LLMs as a reading aid has little downside, with one caveat: mind **data
privacy** when uploading documents to hosted models (ChatGPT, Claude, Gemini,
etc.).
Ensure the provider does not train on your inputs, which often requires opting
out in account settings.

### LLMs as Researchers

With improved sourcing and fewer hallucinations, LLMs work well for light
research where you would otherwise use a search engine.

For deeper research, quality varies widely.
Do not mistake a long, well-formatted report for an accurate one.
Even when a report cites sources, a cited source may be misread, unreliable, or
hallucinated.

Always follow citations and read the primary sources.
Treat LLM research as a starting point, not a finished product.

### LLMs as Editors

LLMs make strong editors when brought in late in the writing process.
Once a draft is written and mostly polished, an LLM can give useful feedback on
structure, phrasing, and consistency without overwriting your voice.

Watch out for sycophancy: LLMs default to praise over critique.
The earlier you involve an LLM, the worse this gets.
Given a rough draft, the model will flatter your ideas while offering to rewrite
the document into something else entirely.

### LLMs as Writers

Although LLMs read and edit well, their writing is weak: cliché-ridden at best,
and full of machine tells at worst.

Generated prose casts doubt on both the writing and the thinking behind it.
When the words look machine-generated, readers question whether the ideas are
too, and disengage.
It also breaks the social contract between writer and reader.
Readers expect the writer to have done the harder intellectual work; with
generated prose, a reader cannot be sure the author even read what they
submitted.

Write prose that represents your own thinking yourself, including RFDs, design
documents, substantive PR descriptions, and important issue writeups.
You can still use an LLM during the writing process, as long as the ideas and
voice remain yours.

For mechanical prose (commit messages, changelog entries, and boilerplate
documentation), LLM generation is fine provided you review the output.

### LLMs as Code Reviewers

LLMs are useful code reviewers when pointed at specific concerns such as error
handling gaps, API misuse, or missed edge cases.
Because they also make nonsensical suggestions and miss broader architectural
problems, LLM review supplements human review rather than replacing it.

### LLMs as Debuggers

LLMs are effective debugging partners.
They act as an interactive [rubber duck][rubber-duck] that can suggest the next
question to ask or hypothesis to test.
Consult an LLM freely when stuck on a bug, as long as it does not replace
talking with teammates.

### LLMs as Programmers

Unlike prose, where LLMs work best as editors of a finished draft, LLMs are
effective at writing code from scratch, especially experimental, auxiliary, or
throwaway code.

Production code requires more care.
Even on tasks well-suited to LLMs, such as writing tests, models can easily
generate plausible-looking nonsense.

**Self-review is essential.** Never submit LLM-generated code for peer review
without reviewing every line yourself.
During peer review, address feedback with targeted edits rather than
regenerating the code from a new prompt; wholesale regeneration obscures what
changed and breaks incremental review.

Do not let a codebase become dependent on an LLM to understand or evolve it.
Never ship code that only an LLM can maintain.

## Anti-Patterns

### LLM Mandates

LLM use is encouraged but never required.
We trust contributors to choose the best tool for the job.

### LLM Shaming

Shaming contributors for using LLMs is equally unwelcome.
As LLMs are built into search, editors, and code completion, drawing a line of
purity is both impractical and counterproductive.
Contributors will make different choices about when and how to use LLMs.

### LLM Anthropomorphization

An LLM is not a person or a team member, and it cannot take responsibility.
Treating it like one (giving it a persona, citing its "opinions," or deferring
to its "judgment") obscures that a human is always accountable for its output.

JP itself is an LLM-based tool with a name and a persona.
The persona is a user interface choice, not an attribution of agency.
JP's output is the user's responsibility.

### Unreviewed Bulk Generation

Submitting large volumes of unreviewed LLM-generated code or prose and expecting
others to debug, maintain, or untangle it is a failure of responsibility.
Like any other unreviewed submission, it shifts the author's work onto the team.

## Guidelines Summary

| Activity              | Guideline                                |
| --------------------- | ---------------------------------------- |
| Reading comprehension | Encouraged. Mind data privacy with       |
|                       | hosted models.                           |
| Research              | Useful starting point. Verify sources.   |
|                       | Don't trust blindly.                     |
| Editing prose         | Encouraged, especially late in the       |
|                       | process.                                 |
| Writing prose         | Generally write it yourself. LLMs fine   |
|                       | for mechanical prose.                    |
| Code review           | Useful supplement. Not a substitute for  |
|                       | human review.                            |
| Debugging             | Use freely. Don't displace               |
|                       | collaboration.                           |
| Writing code          | Encouraged with care. Self-review before |
|                       | peer review. No wholesale regeneration   |
|                       | during review.                           |
| Commit messages       | Fine. Review the output.                 |
| Tests                 | Fine with care. Verify the tests         |
|                       | actually test what they claim.           |

## Determination

The JP project encourages using LLMs.
The tools are capable and improving, and we should use them.

The core contract remains the same: **you own what you ship.** Putting your name
on a commit, review, comment, or document means you stand behind it.
Which tools you used is your business; the quality and correctness of the result
is your responsibility.

## References

- [Oxide RFD 576: Using LLMs at Oxide][oxide-rfd-576]: the direct inspiration
  for this document.
- [RFC 3: Documentation Conventions][rfc-3]: the original spirit of writing
  things down.
- [RFD 001: The JP RFD Process]: how we write and manage RFDs.

[RFD 001: The JP RFD Process]: 001-jp-rfd-process.md
[oxide-rfd-576]: https://rfd.shared.oxide.computer/rfd/0576
[rfc-3]: https://datatracker.ietf.org/doc/html/rfc3
[rubber-duck]: https://en.wikipedia.org/wiki/Rubber_duck_debugging
