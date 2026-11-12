# Shared by `mission.sh`, `next-target`, and `gate`.

# Run `qual` (`crates/internal/qual`) from the worktree root.
#
# This is the only place the mission names the command. `qual report` and
# `qual diff` become rustqual commands when T-1dhk659 folds `qual` into
# rustqual, and this function is then the one line to change.
qual() {
    cargo run --quiet --locked --package qual -- "$@"
}

# Print the path of the guidance file for rule `$1`, relative to the worktree
# root. `qual` names the file after the rule, with `/` replaced by `_`.
guidance_path() {
    printf '.config/rustqual/guidance/%s.md\n' "${1//\//_}"
}
