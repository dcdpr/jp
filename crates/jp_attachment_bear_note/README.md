# Attachment: Bear Note

An attachment handler for the [Bear Note](https://bear.app/) note-taking app
(macOS only).

It allows retrieving the full content of a single note, or a list of notes based
on a search query or tag.

## Usage

The handler supports fetching notes based on three different queries:

- `bear://get/<note-id>`: Fetches a single note by its unique identifier.
- `bear://search/<query>`: Fetches a list of notes matching a search query.
- `bear://tagged/<tag>`: Fetches a list of notes tagged with a specific tag.

```sh
# You can copy the note ID using <kbd>⌥⇧⌘I</kbd>
jp attachment add bear://get/<note-id>
```

```sh
jp attachment add bear://search/my%20query
```

```sh
jp attachment add bear://tagged/my/tag
```
