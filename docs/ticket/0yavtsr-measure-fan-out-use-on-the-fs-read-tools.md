# Measure fan-out use on the fs read tools

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-10-06
- **Implements**: 117
- **Label**: domain=mcp
- **Label**: domain=tooling

RFD 117 adds `fan_out` but enables it for no tool.
Its open question "Does the assistant use it?" is answered by turning it on for
the read-only filesystem tools in this repository's own configuration and
watching what models do.

## Proposal

1. Set `fan_out = true` on `fs_read_file`, `fs_grep_files`, and `fs_list_files`
   in `.jp/mcp/tools/fs/*.toml`.
   These are `run = "allow"` readers, so fan-out adds no prompts. #1183 already
   has the three config changes; the rest of that PR targets the abandoned
   provider-side design and should not be merged as is.
2. Use them for a week and record, per call to those tools, how many operations
   it carried.
   A distribution that stays at one operation per call means the envelope costs
   tokens and buys nothing, and the batching sentence or the schema needs
   another look before fan-out is enabled anywhere else.
3. Note the result in RFD 117's "Does the assistant use it?" risk.

Split out of #1182 review (comment 4195052824): the measurement is a use of the
feature, not part of building it, so it does not hold back RFD 117's Implemented
status.
