#!/usr/bin/env bash
#
# Convert unlinked RFD, ticket, and wiki mentions in Markdown into links.
#
# Reads Markdown from stdin and writes it to stdout, replacing each unlinked
# `RFD 018`, `T-005zd00`, and `W-0a1b2c3` (with an optional `#section`) with a
# link to its short URL on the site:
#
#   See RFD 018#answers and T-005zd00.
#   See [RFD 018#answers](https://jp.computer/rfd-018#answers) and [T-005zd00](https://jp.computer/T-005zd00).
#
# Matches are case-sensitive: prefixes (`RFD`, `T-`, `W-`) must be uppercase
# and IDs must be lowercase, matching the format `jp` produces. `rfd 018` and
# `t-005zd00` are left unchanged.
#
# Fenced code blocks, inline code spans, HTML comments, URLs, and bracketed
# text (link text, reference links, task-list checkboxes) are skipped. This
# makes the filter idempotent, though mentions inside plain square brackets
# also remain unlinked. Indented code blocks are not recognized.
#
# Short URLs are resolved by `redirects.mjs` in `jp-vite`. Mentions are linked
# without checking whether the target document exists, so a typo or a ticket on
# an unmerged branch links to a URL that returns 404 until the page is deployed.
#
# `.github/workflows/link-mentions.yml` runs this script on issue and pull
# request descriptions. To test it against an existing pull request:
#
#   gh pr view 1290 --json body --jq .body | bash .github/link-mentions.sh
#
# Environment:
#
#   JP_SITE_URL  Base URL for generated links (default: https://jp.computer).
#
# Tests: `bash .github/link-mentions-test.sh`.

set -euo pipefail

# `read -d ''` reads until a NUL byte and returns non-zero on EOF.
IFS= read -r -d '' program <<'PERL' || true
use strict;
use warnings;

(my $site = $ENV{JP_SITE_URL} || 'https://jp.computer') =~ s{/+\z}{};

# Markdown spans to pass through unchanged. Uses relative backreferences (\g{-1})
# so group numbers stay valid when embedded in the main loop's regex.
my $kept = qr{
    # Fenced code block (closed by a matching fence or EOF).
    (?m: ^ [ ]{0,3} (`{3,}|~{3,}) .*? (?: ^ [ ]{0,3} \g{-1} [`~]* [ \t\r]* $ | \z ) )
    # HTML comment (closed by --> or EOF).
  | <!-- .*? (?: --> | \z )
    # Inline code span (cannot cross a blank line).
  | (`+) (?: (?! \n [ \t\r]* \n ) . )*? (?<!`) \g{-1} (?!`)
    # Bracketed text (cannot cross a blank line), plus optional `(url)` or `[ref]`.
  | \[ (?: [^\[\]\n] | \n (?! [ \t\r]* \n ) )* \] (?: \( [^()\n]* \) | \[ [^\[\]\n]* \] )?
    # Bare or angle-bracketed URL.
  | <? https?:// [^\s<>]+ >?
}xs;

# Ticket and wiki page ID: 7 lowercase Crockford Base32 characters
# (excludes `i`, `l`, `o`, and `u`).
my $id = qr{[0-9a-hjkmnp-tv-z]{7}};

my $section = qr{(?: \# [\w-]+ )?}x;

# Unlinked `RFD NNN` (allowing a single line break after `RFD`), `T-<id>`, or
# `W-<id>`, each with an optional `#section` anchor.
#
# Capture groups:
#   $1: RFD text (`RFD 018`), $2: RFD number (`018`), $3: RFD `#section`
#   $4: ticket/wiki text (`T-005zd00`), $5: prefix (`T`/`W`), $6: ID, $7: `#section`
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
