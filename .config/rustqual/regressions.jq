# Report every count in a fresh rustqual report that sits above the committed
# baseline, one `key: old -> new (+delta)` line per category. Empty output means
# no category moved up.
#
# Invoked as:
#
#   jq -nr -f regressions.jq --slurpfile old <baseline> --slurpfile new <fresh>
#
# Per category rather than on `total_findings` alone: a change that fixes three
# magic numbers and adds three dead functions leaves the total flat while making
# the codebase worse along an axis we care about.
#
# A baseline that cannot be compared is an error, not a pass. Invalid JSON fails
# in jq before this program runs; a file that parses but carries no numeric
# `total_findings` (`{}`, or a schema from an incompatible rustqual) is rejected
# below. Both would otherwise disable the gate silently.
#
# Keys deliberately not compared:
#
#   version                     schema marker, not a count
#   total                       function count, which grows with the codebase
#   quality_score, iosp_score   ratios, which are what we are not gating on

{version: 1, total: 1, quality_score: 1, iosp_score: 1} as $skip
| $old[0] as $o
| $new[0] as $n
| if ($o | type) != "object" then
    error("baseline is not a JSON object")
  elif ($o.total_findings | type) != "number" then
    error("baseline has no numeric total_findings: corrupt, or written by an incompatible rustqual")
  elif ($n.total_findings | type) != "number" then
    error("fresh report has no numeric total_findings: rustqual's baseline format changed")
  else
    $n
    | to_entries[]
    | select((.value | type) == "number")
    | select($skip[.key] == null)
    | . as $entry
    | ($o[$entry.key] | if type == "number" then . else 0 end) as $was
    | select($entry.value > $was)
    | "  \($entry.key): \($was) -> \($entry.value) (+\($entry.value - $was))"
  end
