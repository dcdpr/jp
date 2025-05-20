commit:
    jp query --no-persist --new --context commit "Generate a commit message" \
    | sed -e 's/\x1b\[[0-9;]*[mGKHF]//g' \
    | git commit --edit --file -
