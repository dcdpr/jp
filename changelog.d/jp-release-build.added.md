`jp release build <version>` runs `plugins.command.release.options.build` to
build release archives, validates `checksums.txt` and `manifest.json`, and
stores the build in workspace user-local storage.
`jp release status` reports finalized releases that do not yet have a
publishable build.
