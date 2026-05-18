# schematic (JP fork)

A layered serde-driven configuration and schema library, embedded as a workspace member.

This is a fork of [moonrepo/schematic](https://github.com/moonrepo/schematic),
last synced from v0.19.4 plus cherry-picks from
[JeanMertz/schematic@merged](https://github.com/JeanMertz/schematic/tree/merged).
It is inlined here so JP can iterate on it without going through upstream;
the feature surface has been (and is being) trimmed to what JP actually uses.

For documentation of the underlying API, see the upstream project at
<https://moonrepo.github.io/schematic>. Note that JP's fork diverges from
upstream — in particular around partial-config generation, untagged enum
support, and the set of supported formats and renderers.

Licensed under MIT (see `LICENSE`), per upstream.
