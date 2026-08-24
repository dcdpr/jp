# xct2cli

Reads an Xcode Instruments `.trace` bundle and produces symbolicated hotspots
and callgraphs.
Apple Silicon only.

`xctrace export` does not hand you symbolicated stacks.
A Time Profiler row carries the leaf symbol, a count of the frames it withheld,
and the rest of the stack as raw ASLR-slid integers fragmented across rows that
back-reference each other by id.
Recovering frames means reassembling those fragments, recovering dyld load
addresses from `kdebug` `DBG_DYLD_UUID_MAP_A`/`_B` tracepoint pairs
cross-referenced against the `kdebug-strings` table, and only then symbolicating
against a dSYM.
This crate does all of it, natively through `addr2line`/`gimli`/`object` rather
than by shelling out to `atos`.

## Provenance

Vendored from <https://github.com/landaire/xct2cli> at commit `9ebb2e0`, MIT OR
Apache-2.0.
Both licence files are kept alongside this one.

Vendored rather than depended on: upstream is one author and a handful of
commits, the code is right, and the problem is ours for years.
That does not remove the maintenance burden.
The `.trace` format is undocumented and remains Apple's to change.

The source is kept close to upstream so it can be re-synced.
Style lints upstream does not satisfy are allowed by name in `Cargo.toml` rather
than fixed, so a diff against a fresh checkout stays readable.

## What differs from upstream

- **Swift demangling.** Upstream carries `rustc-demangle` and `cpp_demangle` and
  no Swift demangler, so every Swift symbol reports as `$s2JP17Conversation…`.
  `symbol::swift` loads `libswiftDemangle.dylib` out of the active Xcode
  toolchain with `dlopen` and calls it from the single `demangle` choke point in
  `symbol::macho`.
  Point `SWIFT_DEMANGLE_DYLIB` at a specific copy to override the search.
  A missing dylib degrades to the mangled name.
- **quick-xml 0.41.** Upstream targets 0.39, where `xml_content` and
  `normalized_value` take no XML version.
  The version is threaded through `xml::XML_VERSION`.
- **Environment redaction.** `redact::strip_environment` runs over everything
  `xctrace` produces, at the single point in `xctrace::Xctrace` where its output
  is returned.
  See "A recorded trace is a secret" below.
- **Removed:** the `xct2cli` binary and its `cli` feature; `render`, which
  formatted reports for a terminal; `analysis::annotate`, which annotated
  disassembly; `analysis::{pmi, counters}`, which read hardware performance
  counters; and `Xctrace::record_launch`, which started a recording.
  Dropping the first three also drops `capstone`, `annotate-snippets`, and
  `owo-colors`.
  The counter paths needed kperf, which needs root or
  `com.apple.private.kernel.kpc`.
  Recording is spawned by the caller, which is what lets it use `--instrument`;
  `record_launch` passed `--template`, and it carried the only code that built
  an `--env` argument, so nothing here can put a credential on a command line.

## Recording a trace it can read

Debug builds keep debug info in the object files, so the app must be built with
`DEBUG_INFORMATION_FORMAT = dwarf-with-dsym` (set for Debug in
`apps/macos/project.yml`).

```sh
just build-app Debug
xcrun xctrace record --instrument 'Time Profiler' \
  --time-limit 10s --output /tmp/jp.trace \
  --launch -- /path/to/JP.app
```

Three parts of that command are easy to get wrong.

**`--instrument`, not `--template`.** On Xcode 26 a template produces a trace
that fails export with "Document Missing Template Error".

**`--launch`, not `--attach`.** Slide recovery reads dyld image loads out of the
`dyld-library-load` and `kdebug` tables, and those only record loads that happen
*during* the window.
A process you attach to loaded its libraries before recording started, so the
tables come back empty, no slide can be recovered, and nothing symbolicates.

**The `--` is required.** `--launch -- <path>` execs the path.
Without the `--`, xctrace resolves the argument as a name and fails with
"Provided process is ambiguous" whenever more than one copy of the bundle exists
on the machine.

## A recorded trace is a secret

A `.trace` bundle embeds the full environment of the process it recorded, and
`xctrace export --toc` prints it.
Recording against a shell-launched process captures every API key and token that
shell exported.

This crate strips `<environment>` from everything it reads back, so using the
library cannot disclose them.
That does not sanitise the bundle: the values are still on disk inside it, and
anyone running `xctrace export` by hand will see them.
Never commit a recorded bundle, and treat one on disk as credential material.

The fixtures under `tests/fixtures` are upstream's exported XML, not bundles,
which is why they are safe to keep.
