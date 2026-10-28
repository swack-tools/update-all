## update-all @VERSION@

update-all checks everything on an Apple silicon Mac for updates, shows how far
behind each item is, and updates what you choose. It never installs anything
that isn't already installed.

### Requirements

- macOS on Apple silicon. update-all targets the current macOS release and the
  current beta.

### Install

1. Download `@ZIP@`.
1. Unzip it, then move `update-all` to a folder on your `PATH`, such as
   `/usr/local/bin`.
1. Run `update-all --dry-run` to preview updates, or `update-all --help` for
   the full guide.

<!-- unsigned -->
### Before the first run

Apple hasn't notarized this build, so macOS blocks it the first time if you
downloaded it with a browser. To allow it, run this command:

```sh
xattr -d com.apple.quarantine /usr/local/bin/update-all
```
<!-- /unsigned -->
<!-- notarized -->
### Signed and notarized

This build carries a Developer ID signature, and Apple notarized it, so macOS
runs it without a warning.
<!-- /notarized -->

### What's included

- `@ZIP@`: the `update-all` binary, `README.md`, `LICENSE`, and the example
  `config.toml`.
- `@ZIP@.sha256`: the checksum. To check the download, run
  `shasum -a 256 -c @ZIP@.sha256`.

### License

GNU General Public License v3.0. See `LICENSE`.
