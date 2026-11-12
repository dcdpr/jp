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
