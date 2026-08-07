# RFD 002: Using LLMs in the JP Project

- **Status**: Implemented
- **Category**: Process
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-17

## Summary

LLM usage is encouraged in the JP project.
Contributors are free to use LLMs for reading, research, editing, debugging,
code review, and code generation.
All work product — code, prose, reviews, comments — is the responsibility of
the person who submits it, regardless of how it was produced.

## The Core Principle

**You own what you ship.** Every commit, comment, review, and message carries
the name of the person who submitted it.
That attribution is a statement of responsibility.
If you use an LLM to help produce an artifact, the artifact is yours — you
reviewed it, you stand behind it, you will maintain it.
"An LLM wrote it" is not a defense, an excuse, or a disclaimer.
It is irrelevant.

This is not a burden.
It is the same standard that applies to any tool: a compiler, a linter, a code
generator, a Stack Overflow answer, a colleague's suggestion.
The tool helps; you decide.

## Values

Several of our values inform how we think about LLM use.
Listed here in priority order:

- **Responsibility**: Our lodestar.
  However powerful they may be, LLMs are a tool, acting at the behest of a
  human.
  Contributors bear responsibility for the artifacts they create, whatever
  automation was employed to create them.
  Human judgment remains firmly in the loop: even when an LLM generates an
  artifact we will use (code, tests, documentation, prose), the output is the
  responsibility of the human using it.

- **Rigor**: LLMs are double-edged with respect to rigor.
  Wielded carefully, they can sharpen our thinking by pointing out holes in
  reasoning or providing thought-provoking suggestions.
  Used recklessly, they replace crisp thinking with generated flotsam.
  LLMs are useful in as much as they promote and reinforce our rigor.

- **Empathy**: Be we readers or writers, there are humans on the other end of
  our communication.
  As we use LLMs, we must keep in mind our empathy for that human — the one
  consuming our writing, reviewing our code, or reading our commit messages.

- **Teamwork**: We are working together on a shared endeavor.
  LLM use must not undermine the trust we have in one another.
  In some contexts, LLM usage is expected (using an LLM to help debug a tricky
  issue is natural); in others, it can erode trust (submitting LLM-generated
  code for review without having reviewed it yourself shifts the burden to your
  reviewer).
  The distinction is not about disclosure — it is about whether you have taken
  responsibility for the work.

- **Urgency**: LLMs afford an opportunity to do work more quickly, but pace must
  not come at the expense of responsibility, rigor, empathy, and teamwork.
  Too many projects have treated LLMs as an opportunity to increase velocity
  over all else, without regard for direction or quality.

## Uses of LLMs

LLM use varies widely, and the implications vary accordingly.

### LLMs as Readers

LLMs are superlative at reading comprehension, able to process and meaningfully
comprehend documents effectively instantly.
This can be powerful for summarizing documents, answering specific questions
about a specification, or understanding an unfamiliar codebase.

Using LLMs to assist comprehension has little downside.
One caveat: when uploading a document to a hosted LLM (ChatGPT, Claude, Gemini,
etc.), be mindful of **data privacy**.
Ensure the model will not use the document to train future iterations.
This is often opt-out and controlled via account preferences.

### LLMs as Researchers

LLMs are well-suited for the kind of light research tasks for which one would
historically use a search engine, especially as hallucination has been
attenuated and sourcing has improved.

For more involved research, the quality of results can vary widely.
One should be careful about drawing too much confidence from the lengthy,
well-formatted nature of an LLM-powered report.
Even if a report appears well-sourced, the sources themselves may be incorrect
— a seemingly authoritative source found by an LLM may itself be an LLM
hallucination.

When engaging LLMs as researchers, follow citation links to learn from the
original sources.
Treat LLM-researched content as a jumping-off point, not a finished product.

### LLMs as Editors

LLMs can be excellent editors.
Engaging an LLM late in the writing process — with a document already written
and broadly polished — allows for helpful feedback on structure, phrasing, and
consistency without danger of losing one's own voice.

A cautionary note: LLMs are infamous pleasers.
The breathless praise from an LLM is often more sycophancy than analysis.
This becomes more perilous the earlier one uses an LLM in the writing process:
the less polish a document already has, the more likely the LLM will steer
toward something wholly different — praising your genius while offering to
rewrite it for you.

### LLMs as Writers

An LLM can write project documents, including RFDs, when the contributor
supplies the decisions and understands them.
JP's own RFD workflow works this way.
Contributor and assistant discuss the problem, the contributor settles the
scope, and the assistant then writes the document from that discussion.

The line is between delegating the typing and delegating the thinking.
The first is fine.
The second is the failure this section exists to prevent.

Generated prose fails in specific ways, and each has a countermeasure:

- **It hides an unsettled decision.** Fluent prose about an open question reads
  like a resolved one.
  Settle the scope in conversation before asking for a draft.
- **It manufactures agreement.** A model will write "we chose X because Y" for
  an X nobody chose.
  Read the result against what was actually discussed.
- **It grows.** Every review round invites another caveat, and a long document
  is not evidence that anyone resolved the design.
  The prose budgets and the bounded review cycle in [RFD 001] exist for this.
- **It carries tells.** Machine-generated prose is cliché-ridden by default, and
  readers disengage once they notice.
  The countermeasure is the project's voice rules applied to generated text, not
  abstention.

The contributor owns the problem, the decisions, and the final approval.
The assistant writes and revises the prose.
Before approving, the contributor reads the result and confirms it says what was
agreed.

**Automated review does not substitute for that approval.** Two models clearing
a document tells you they agree, which is not the same as the document being
right.

For mechanical prose, such as commit messages, changelog entries, and
boilerplate documentation, LLM generation is fine when you check the output.

### LLMs as Code Reviewers

LLMs can make for good code reviewers, especially when targeted to look for a
particular kind of issue (error handling gaps, API misuse, missed edge cases).
But they can also make nonsense suggestions or miss larger architectural
problems.
LLM review is a supplement, not a substitute for human review.

### LLMs as Debuggers

LLMs can be surprisingly helpful debugging problems.
They serve as a kind of animatronic [rubber duck][rubber-duck], helping inspire
the next questions to ask.
When debugging a vexing problem, one has little to lose by consulting an LLM —
provided it does not displace collaboration with colleagues.

### LLMs as Programmers

LLMs are remarkably good at writing code.
Unlike prose (which should be handed in polished form to an LLM to maximize its
efficacy as an editor), LLMs can be quite effective writing code *de novo*.
This is especially valuable for code that is experimental, auxiliary, or
throwaway.

The closer code is to the system we ship, the greater care needs to be shown.
Even with tasks that seem natural for LLM contribution (e.g., writing tests),
one should still be careful: it is easy for LLMs to spiral into nonsense on even
simple tasks.

Where LLM-generated code is used, **self-review is essential**.
LLM-generated code should not be submitted for peer review if the responsible
engineer has not themselves reviewed it.
Once in the loop of peer review, wholesale re-generation in response to review
comments makes iterative review impossible — the reviewer cannot see what
changed.
Address review comments with targeted edits, not by re-prompting.

Using LLMs to aid in programming requires judgment and balance.
They can be extraordinarily useful across the entire spectrum of software
activity and should not be dismissed out of hand.
But any implicit dependency on an LLM to comprehend or evolve a system must be
resisted: we do not want to produce code that only an LLM can maintain.

## Anti-Patterns

### LLM Mandates

LLM use is encouraged but never required.
As with any tool, contributors should feel empowered to use LLMs, but not
obligated to do so.
We trust each other to choose the best tool for the job.

### LLM Shaming

Conversely, shaming others for using LLMs is equally unwelcome.
As LLMs become the foundation for technologies like search and code completion,
drawing a bright line of purity becomes increasingly impractical — and
counterproductive.
People will make different choices about when and how to use LLMs.
That's fine.

### LLM Anthropomorphization

An LLM is not a person.
It cannot be held accountable, cannot take responsibility, and cannot be a team
member.
Treating it as one — giving it a persona, referring to its "opinions,"
deferring to its "judgment" — obscures the fact that a human is always
responsible for what the LLM produces.

Yes, JP itself is an LLM-based tool with a name and a persona.
The irony is not lost on us.
The persona is a user interface choice, not an attribution of agency.
JP's output is the user's responsibility.

### Undisclosed Bulk Generation

Submitting large volumes of LLM-generated code or prose without review, then
expecting others to debug, maintain, or untangle it, is a failure of
responsibility.
The problem is not that an LLM was used — it is that the submitter did not take
ownership of the output.
The result is the same as submitting any other unreviewed work: it creates a
burden for the team.

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
|                       | peer review. No wholesale re-generation  |
|                       | during review.                           |
| Commit messages       | Fine. Review the output.                 |
| Tests                 | Fine with care. Verify the tests         |
|                       | actually test what they claim.           |

## Determination

LLM use is encouraged in the JP project.
The tools are powerful and getting better.
We should use them.

But the fundamental contract does not change: **you own what you ship.** Your
name on a commit, a review, a comment, or a document means you stand behind it.
The tool you used to produce it is your business.
The quality and correctness of the result is your responsibility.

## References

- [Oxide RFD 576: Using LLMs at Oxide][oxide-rfd-576] — the direct inspiration
  for this document.
- [RFC 3: Documentation Conventions][rfc-3] — the original spirit of writing
  things down.
- [RFD 001: The JP RFD Process] — how we write and manage RFDs.

[RFD 001: The JP RFD Process]: 001-jp-rfd-process.md
[RFD 001]: 001-jp-rfd-process.md
[oxide-rfd-576]: https://rfd.shared.oxide.computer/rfd/0576
[rfc-3]: https://datatracker.ietf.org/doc/html/rfc3
[rubber-duck]: https://en.wikipedia.org/wiki/Rubber_duck_debugging
