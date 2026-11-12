# Shared by `mission.sh`, `next-target`, and `gate`.

# Run `qual` (`crates/internal/qual`) from the worktree root.
#
# Centralizing the invocation here keeps the switch to `rustqual` (T-1dhk659) to
# a single line.
qual() {
    cargo run --quiet --locked --package qual -- "$@"
}

# Print the path of the guidance file for rule `$1`, relative to the worktree
# root. `qual` names the file after the rule, with `/` replaced by `_`.
guidance_path() {
    printf '.config/rustqual/guidance/%s.md\n' "${1//\//_}"
}

# Read a unified diff on stdin and print each `qual:` annotation line it adds or
# changes, sorted and without indentation.
#
# Annotation lines are compared as a multiset. A line the diff removes and adds
# again, as when an item moves with its marker, is not printed. Editing a
# marker, such as raising `N` in `qual:allow(srp, file_length=N)`, removes one
# line and adds a different one, so the added line is printed.
#
# Any `qual:<name>` counts, whichever markers rustqual defines: `qual:allow`,
# `qual:api`, `qual:test_helper`, `qual:inverse`, `qual:recursive`.
new_markers() {
    jq -Rrn '
        reduce (
            inputs
            | select(test("^[-+]") and (test("^(---|\\+\\+\\+) ") | not) and test("qual:[a-z_]+"))
            | {count: (if startswith("+") then 1 else -1 end),
               text: (.[1:] | sub("^\\s+"; "") | sub("\\s+$"; ""))}
        ) as $line ({}; .[$line.text] += $line.count)
        | keys[] as $text
        | select(.[$text] > 0)
        | $text
    '
}
