# Attachment: Bear Note

An attachment handler for the [Bear Note](https://bear.app/) note-taking app
(macOS only).

It allows retrieving the full content of a single note, or a list of notes based
on a search query, optionally filtered by tags.

## Usage

Fetch a single note by its unique identifier:

```sh
# You can copy the note ID using <kbd>⌥⇧⌘I</kbd>
jp attachment add "bear://get/2356A6D7-49D7-4818-8E37-3E02D1B95146"
```

Fetch a list of notes based on a search query:

```sh
# path and query is URL-encoded
jp attachment add "bear://search/my query"
```

Fetch a list of notes based on a search query, filtered by tags:

```sh
jp attachment add "bear://search/my query?tag=foo&tag=bar"
```

Fetch a list of notes tagged with a specific tag:

```sh
jp attachment add "bear://search/?tag=project/my-project"
```

List all added URIs:

```sh
jp attachment ls

Attachments:
  bear://get/2356A6D7%2D49D7%2D4818%2D8E37%2D3E02D1B95146
  bear://search/?tag=project%2Fmy%2Dproject
  bear://search/my%20query
  bear://search/my%20query?tag=foo&tag=bar
```
