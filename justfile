commit:
    #!/usr/bin/env sh
    if message=$(jp query --no-persist --new --context=commit --no-edit); then
        echo "$message" | sed -e 's/\x1b\[[0-9;]*[mGKHF]//g' | git commit --edit --file=-
    fi

docs CMD="dev" *FLAGS:
    #!/usr/bin/env sh
    cd docs
    yarn vitepress {{CMD}} {{FLAGS}}
