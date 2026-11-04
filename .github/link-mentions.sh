#!/usr/bin/env bash
#
# Link the RFDs, tickets, and wiki pages a description names without linking.
#
# Reads Markdown on stdin and writes it to stdout with each unlinked `RFD 018`,
# `T-005zd00`, and `W-0a1b2c3`, optionally followed by a `#section`, made a link
# to its short link on the site:
#
#   See RFD 018#answers and T-005zd00.
#   See [RFD 018#answers](https://jp.computer/rfd-018#answers) and [T-005zd00](https://jp.computer/T-005zd00).
#
# Only the spelling `jp` writes is linked: `RFD`, `T-`, and `W-` in upper case,
# ids in lower case. `rfd 018` and `t-005zd00` are left as they are.
#
# Fenced code blocks, code spans, HTML comments, URLs, and text in square
# brackets (link text, reference links, task-list boxes) are left as they are.
# That makes a second run change nothing; it also means a mention inside plain
# square brackets stays unlinked. Indented code blocks are not recognised.
#
# The site resolves the short links (`redirects.mjs` in jp-vite). A mention
# with no document behind it, such as a typo or a ticket that only exists on an
# unmerged branch, still gets a link, to a page that 404s until it exists.
#
# `.github/workflows/link-mentions.yml` runs this on issue and pull request
# descriptions. To try it on one:
#
#   gh pr view 1290 --json body --jq .body | bash .github/link-mentions.sh
#
# Environment:
#
#   JP_SITE_URL  the site the links point at. Defaults to https://jp.computer.
#
# Tests: `bash .github/link-mentions-test.sh`.

set -euo pipefail

# `read` reports the end of input it stops at as a failure.
IFS= read -r -d '' program <<'PERL' || true
use strict;
use warnings;

(my $site = $ENV{JP_SITE_URL} || 'https://jp.computer') =~ s{/+\z}{};

# What is left as it is. The backreferences are relative, so they keep pointing
# at the right group inside the larger pattern the main loop builds.
my $kept = qr{
    # A fenced code block, to its closing fence or the end of the text.
    (?m: ^ [ ]{0,3} (`{3,}|~{3,}) .*? (?: ^ [ ]{0,3} \g{-1} [`~]* [ \t\r]* $ | \z ) )
    # An HTML comment.
  | <!-- .*? (?: --> | \z )
    # A code span, which a blank line ends.
  | (`+) (?: (?! \n [ \t\r]* \n ) . )*? (?<!`) \g{-1} (?!`)
    # Text in square brackets, which a blank line ends, with the destination or
    # label after it.
  | \[ (?: [^\[\]\n] | \n (?! [ \t\r]* \n ) )* \] (?: \( [^()\n]* \) | \[ [^\[\]\n]* \] )?
    # A URL.
  | <? https?:// [^\s<>]+ >?
}xs;

# Ticket and wiki page ids: seven characters of Crockford's base 32, which
# leaves out `i`, `l`, `o`, and `u`.
my $id = qr{[0-9a-hjkmnp-tv-z]{7}};

my $section = qr{(?: \# [\w-]+ )?}x;

# `RFD 018`, its space possibly a line break, or `T-005zd00` or `W-0a1b2c3`,
# each with an optional section.
#
# Groups: the RFD's text, its number, and its section; the ticket or page's
# text, its prefix, its id, and its section.
my $mention = qr{
    (?<! [\w/-] )
    (?: ( RFD (?: [ \t]* \r? \n (?! [ \t\r]* \n ) [ \t]* | [ \t]+ ) ( \d{3} ) ) (?! \w ) ( $section )
      | ( ([TW]) - ($id) ) (?! [\w-] ) ( $section ) )
}x;

sub linked {
    my ($text) = @_;
    $text =~ s{$mention}{
        defined $2 ? "[$1$3]($site/rfd-$2$3)" : "[$4$7]($site/$5-$6$7)"
    }ge;

    return $text;
}

my $text = do { local $/; <STDIN> } // '';
my $out = '';
while ($text =~ /\G(.*?)($kept)/gcs) {
    my ($plain, $as_is) = ($1, $2);
    $out .= linked($plain) . $as_is;
}
$out .= linked(substr($text, pos($text) // 0));

print $out;
PERL

exec perl -CSD -e "$program"
