#!/usr/bin/env bash
#
# Tests for `link-mentions.sh`.
#
# Each case checks the exact output, and that running the script on that output
# changes nothing: a description is linked again every time it is edited.
#
# Run: bash .github/link-mentions-test.sh

set -uo pipefail

script="$(dirname "$0")/link-mentions.sh"
failures=0

link() {
    printf '%s' "$1" | JP_SITE_URL=https://jp.computer bash "$script"
}

check() { # name input expected
    local once twice
    once=$(link "$2")
    twice=$(link "$once")

    if [[ $once != "$3" ]]; then
        failures=$((failures + 1))
        printf 'FAIL %s\n  input:    %q\n  expected: %q\n  actual:   %q\n' "$1" "$2" "$3" "$once"
    elif [[ $twice != "$once" ]]; then
        failures=$((failures + 1))
        printf 'FAIL %s: a second run changed it\n  first:  %q\n  second: %q\n' "$1" "$once" "$twice"
    fi
}

check 'an RFD' \
    'See RFD 018.' \
    'See [RFD 018](https://jp.computer/rfd-018).'

check 'a ticket and a wiki page' \
    'Fixes T-005zd00, W-0a1b2c3.' \
    'Fixes [T-005zd00](https://jp.computer/T-005zd00), [W-0a1b2c3](https://jp.computer/W-0a1b2c3).'

check 'sections' \
    'RFD 018#answers and W-0a1b2c3#turn.' \
    '[RFD 018#answers](https://jp.computer/rfd-018#answers) and [W-0a1b2c3#turn](https://jp.computer/W-0a1b2c3#turn).'

check 'an RFD wrapped onto the next line' \
    $'as RFD\n018 says' \
    $'as [RFD\n018](https://jp.computer/rfd-018) says'

check 'emphasis and parentheses' \
    '**T-005zd00** (see RFD 018)' \
    '**[T-005zd00](https://jp.computer/T-005zd00)** (see [RFD 018](https://jp.computer/rfd-018))'

check 'a heading and a task list' \
    $'## RFD 018\n\n- [ ] T-005zd00\n- [x] W-0a1b2c3' \
    $'## [RFD 018](https://jp.computer/rfd-018)\n\n- [ ] [T-005zd00](https://jp.computer/T-005zd00)\n- [x] [W-0a1b2c3](https://jp.computer/W-0a1b2c3)'

check 'links' \
    '[RFD 018](https://jp.computer/rfd-018), [the ticket][T-005zd00], and [W-0a1b2c3]' \
    '[RFD 018](https://jp.computer/rfd-018), [the ticket][T-005zd00], and [W-0a1b2c3]'

check 'a reference definition' \
    $'See [RFD 018].\n\n[RFD 018]: https://jp.computer/rfd/018-prompts-as-inquiry-events' \
    $'See [RFD 018].\n\n[RFD 018]: https://jp.computer/rfd/018-prompts-as-inquiry-events'

check 'URLs' \
    'https://jp.computer/T-005zd00 and <https://jp.computer/W-0a1b2c3>' \
    'https://jp.computer/T-005zd00 and <https://jp.computer/W-0a1b2c3>'

check 'code spans' \
    '`T-005zd00` and ``RFD 018``' \
    '`T-005zd00` and ``RFD 018``'

check 'an unclosed code span' \
    'a ` and T-005zd00' \
    'a ` and [T-005zd00](https://jp.computer/T-005zd00)'

check 'fenced code blocks' \
    $'RFD 018\n```sh\njp ticket show T-005zd00\n```\n~~~\nW-0a1b2c3\n~~~\nW-0a1b2c3' \
    $'[RFD 018](https://jp.computer/rfd-018)\n```sh\njp ticket show T-005zd00\n```\n~~~\nW-0a1b2c3\n~~~\n[W-0a1b2c3](https://jp.computer/W-0a1b2c3)'

check 'an unclosed fence' \
    $'RFD 018\n```\nT-005zd00' \
    $'[RFD 018](https://jp.computer/rfd-018)\n```\nT-005zd00'

check 'an HTML comment' \
    $'<!-- RFD 018 -->\nRFD 018' \
    $'<!-- RFD 018 -->\n[RFD 018](https://jp.computer/rfd-018)'

check 'CRLF line endings' \
    $'```\r\nRFD 018\r\n```\r\nRFD 018' \
    $'```\r\nRFD 018\r\n```\r\n[RFD 018](https://jp.computer/rfd-018)'

# Prefixes are upper case and ids lower case, as `jp` writes them.
check 'text that names nothing' \
    'T-shirts, T-0123456789, xT-005zd00, docs/T-005zd00, RFD 0180, RFD D12, rfd 018, t-005zd00, w-0a1b2c3, T-005ZD00' \
    'T-shirts, T-0123456789, xT-005zd00, docs/T-005zd00, RFD 0180, RFD D12, rfd 018, t-005zd00, w-0a1b2c3, T-005ZD00'

# Plain square brackets read like link text, so what is in them stays as it is.
check 'a mention in plain brackets' \
    '[see T-005zd00]' \
    '[see T-005zd00]'

check 'a blank line ends an open bracket' \
    $'[draft\n\nT-005zd00' \
    $'[draft\n\n[T-005zd00](https://jp.computer/T-005zd00)'

check 'an empty description' '' ''

if ((failures > 0)); then
    echo "$failures failed."
    exit 1
fi
echo "All passed."
