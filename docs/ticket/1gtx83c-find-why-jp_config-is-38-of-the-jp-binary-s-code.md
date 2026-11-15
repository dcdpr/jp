# Find why jp_config is 38% of the jp binary's code

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-10
- **Label**: package=jp_config
- **Label**: type=task

In a default `release` build of `jp` for aarch64-apple-darwin, `cargo bloat`
attributes 17.4 MiB of the binary's 45.5 MiB of code to `jp_config`: 38%, more
than twice `jp_cli` and almost seven times `jp_llm`.

```text
$ cargo bloat --release -p jp_cli --bin jp --crates
 File  .text     Size Crate
20.1%  38.3%  17.4MiB jp_config
 9.0%  17.0%   7.8MiB jp_cli
 3.0%   5.7%   2.6MiB jp_llm
 2.5%   4.8%   2.2MiB std
 ...
52.6% 100.0%  45.5MiB .text section size, the file size is 86.5MiB
```

`cargo bloat` calls its crate attribution guesswork, so the first step is
confirming where the code comes from.
The likely source is code generated for each config type. 93 types derive
`schematic::Config`, which generates a `Partial*` type with its own serde and
merge code, and `Schematic` schema impls for both types; most also implement
some of `AssignKeyValue`, `PartialConfigDelta`, `FillDefaults`, and `ToPartial`
by hand.
A generic function's code is counted in the crate that instantiates it, so
serde's deserializers for those types count as `jp_config`'s.

With the profile JP's release binaries are planned to use (`opt-level = "s"`,
fat LTO, one codegen unit, symbols stripped), the macOS arm64 binary is 32 MB
instead of 88 MB.
How much of those 32 MB is `jp_config`'s is not measured.

## Work

- [ ] Find the largest functions attributed to `jp_config` (`cargo bloat
      --release -p jp_cli --bin jp --filter jp_config -n 50`) and the most
      instantiated generic functions (`cargo llvm-lines -p jp_config`).
- [ ] Attribute them to their source: schematic's derive, serde, JP's merge
      traits, or hand-written code.
- [ ] Reduce what can be reduced, measuring the release binary's size and
      `jp_config`'s compile time (`cargo build --timings`) before and after.
