# Decode strict-tool omissions through references and composed schemas

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-11
- **Label**: domain=llm
- **Label**: package=jp_llm
- **Label**: type=follow-up

Strict-provider tool argument decoding currently covers inline properties,
nested objects, array items, and a simple anyOf nullable wrapper.
It deliberately leaves decoding paths through `$ref`, `allOf`, `oneOf`, and
general `anyOf` unions untouched.
An optional inline array whose item schema uses `$ref` can still be omitted; the
gap is decoding within the referenced schema or an optional property itself
declared through a reference.

Address this in a separate PR that reviews reference resolution and composition
handling together across the existing schema reader and strict-provider
conversion.
Do not introduce a second reference resolver solely for argument decoding.

Preserve source-accepted null values and required fields.
Record omission instructions only where the outgoing provider encoding actually
introduces an omission placeholder.
Cover shared and recursive definitions, reference siblings, escaped JSON
pointers, and branch-dependent property requirements.
Keep the source schema unchanged and pin provider schema output alongside
decoded argument outcomes.

The existing strict converter also cannot inject nullability into a standalone
`$ref` or an untyped general union.
Review that encoding gap together with decoding rather than treating every null
at a referenced optional property as omission.

Related: T-063sw1z (shared argument validation).
