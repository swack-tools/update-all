# update-all

update-all checks everything on an Apple silicon Mac for updates, shows how far
behind each item is, and updates what you choose. It's one binary with no
runtime dependencies, and it never installs anything that isn't already
installed.

## What it checks

| Source | What it covers | How it updates |
|---|---|---|
| Homebrew | Formulae and casks | `brew upgrade` |
| Package managers | Global npm, pipx, uv, Cargo, Bun, and gem packages, rustup toolchains, and standalone uv, Bun, and Deno | Each package manager's own command |
| pip | Packages in Homebrew's Python | Never. update-all lists them only, because pip upgrades can break Homebrew's Python. |
| AI command-line tools | Claude Code, Antigravity CLI, opencode, GitHub Copilot CLI, goose, Kiro CLI, Amp, and Droid when they installed themselves | Each tool's own update command |
| Apps and browsers | Browsers, AI desktop apps, and developer apps that Homebrew doesn't manage | The vendor's own updater, such as GoogleUpdater for Chrome, a built-in Sparkle updater for Sparkle apps, and msupdate for Microsoft apps |
| Electron apps | Apps built on Electron, such as Claude, Signal, and Obsidian | Opt-in. Homebrew for its casks, otherwise a background launch so the app's own updater runs |
| App Store | Mac App Store apps, including Xcode | Opt-in. The App Store's own framework, with the Updates page as a fallback |
| Developer tools | Extensions for VS Code and editors built on it, gh extensions, and Google Cloud CLI components | Each tool's own command |
| Docker images | Tags that now point to a newer image in the registry | `docker pull`. Containers keep running. |
| Helm charts | Releases with newer charts | Never. update-all prints a `helm upgrade` command for each release so you can review it and run it yourself. |
| Shell frameworks | Oh My Zsh, Prezto, zinit, antidote, Antigen, zplug, Zim, sheldon, zgenom, Fisher, Oh My Fish, Oh My Bash, Bash-it, tpm, and more | Opt-in on the first-run screen. Each framework's own update command. |
| macOS | macOS, Safari, and Command Line Tools | `softwareupdate`. Updates that need a restart run last, and only after you confirm. |

Tools that aren't installed stay silent. update-all doesn't mention them, and
it never offers to install them.

With `--update-literally-all`, update-all also checks every other app in
`/Applications` and `~/Applications`. Apps without an update channel that
update-all can drive appear in a list of apps to update yourself.

## Requirements

- macOS on Apple silicon: the current release or the current beta
- Rust 1.88 or later to build it

## Download

Each [release](https://github.com/swack-tools/update-all/releases) has a zip
with the `update-all` binary, this README, the license, and the example
config, plus a checksum file. Unzip it and move `update-all` to a folder on
your `PATH`, such as `/usr/local/bin`.

## Build and install

```sh
cargo build --release
cp target/release/update-all /usr/local/bin/
```

To install it into `~/.cargo/bin` instead, run `cargo install --path .`.

## Usage

```sh
update-all            # check everything, then answer yes, no, or pick
update-all --dry-run  # show what would update and the exact commands
update-all --yes      # update everything without asking
```

| Option | What it does |
|---|---|
| `-n`, `--dry-run` | Shows what would update and the commands a real run would use, then stops. It changes no installed software, but it still refreshes package lists such as Homebrew's so the preview is accurate. |
| `-y`, `--yes` | Answers yes to every question except the restart question. |
| `--update-literally-all` | Also checks every other app in `/Applications` and `~/Applications`. |
| `--electron` | Includes Electron apps, which a run leaves out by default. |
| `--electron-only` | Checks and updates only Electron apps. |
| `--app-store` | Includes App Store apps, which a run leaves out by default. |
| `--app-store-only` | Checks and updates only App Store apps. |
| `--configure-shells` | Opens the shell framework screen again. |
| `--config PATH` | Reads a different config file. |
| `-h`, `--help` | Shows a short help. `--help` shows the full guide. |
| `-V`, `--version` | Shows the version. |

`--electron-only` and `--app-store-only` work together, so passing both
checks only those two groups.

## How a run works

1. update-all checks every source at the same time.
1. It lists everything that's out of date, grouped by source, with the
   installed and newest versions and their release dates.
1. When Electron apps or App Store apps have updates, it asks whether to
   include each group.
1. You answer `y` to update everything listed, `n` to stop, or `p` to pick.
1. Updates run in order, starting with Homebrew because other tools depend on
   it.
1. Updates that need a restart run last. update-all lists them and asks
   before it installs them and restarts the Mac. After the restart, run
   update-all again, because some updates appear only after others install.

### The picker

The picker lists every update with its path, what it does, both versions,
both release dates, and the number of days between those dates. The most out-of-date
items come first.

| Key | Action |
|---|---|
| Space | Toggle the selected item |
| `a` | Select every visible item |
| `n` | Clear every visible item |
| `/` | Filter by name, ID, path, or description |
| Enter | Start the updates |
| `q` or Esc | Quit without changes |

### Days behind

The Days column shows how many days apart the installed release and the
newest release are.

- Yellow means 7 days or more.
- Orange means 14 days or more.
- Red means 30 days or more.

A value like `~12` means update-all knows only the newest release date. The
number then counts the days since that release, which is the least the item
can be behind. A dash means update-all knows neither date.

## Configuration

update-all reads `~/.config/update-all/config.toml` at startup, or the file you
pass with `--config`. Ideally the file is empty or missing. The
[example config](config.toml) has every option commented out.

The file is TOML. It holds `ignore_paths`, a list of folders that update-all
never looks in, and one section for each kind of item. Each section accepts
two settings:

| Setting | What it does |
|---|---|
| `skip` | Items that update-all never checks. |
| `pin` | Items to hold at a version or version prefix. A pinned item updates only when the new version matches the pin. |

| Section | Kind of item | Example |
|---|---|---|
| `[brew]` | Homebrew formulae | `skip = ["wget"]` |
| `[cask]` | Homebrew casks | `skip = ["firefox"]` |
| `[npm]` | Global npm packages | `pin = { typescript = "5" }` |
| `[pipx]` | pipx apps | `skip = ["black"]` |
| `[uv]` | uv tools | `skip = ["ruff"]` |
| `[cargo]` | Binaries from `cargo install` | `skip = ["ripgrep"]` |
| `[bun]` | Global Bun packages | `skip = ["prettier"]` |
| `[gem]` | Ruby gems | `skip = ["rails"]` |
| `[rustup]` | rustup toolchains | `skip = ["stable-aarch64-apple-darwin"]` |
| `[self]` | Standalone uv, Bun, and Deno | `skip = ["deno"]` |
| `[ai]` | AI command-line tools | `skip = ["claude"]` |
| `[app]` | Apps in `/Applications` and `~/Applications` | `pin = { "Visual Studio Code" = "1.95.3" }` |
| `[appstore]` | App Store apps | `skip = ["Xcode"]` |
| `[ext]` | Editor extensions, written as `editor:extension` | `skip = ["code:ms-python.python"]` |
| `[gh]` | gh extensions | `skip = ["copilot"]` |
| `[gcloud]` | Google Cloud CLI components | `skip = ["bq"]` |
| `[docker]` | Docker image tags. Accepts `skip` only. | `skip = ["postgres:16"]` |
| `[shell]` | Shell frameworks. Accepts `skip` only. | `skip = ["oh-my-zsh"]` |
| `[macos]` | macOS, Safari, and Command Line Tools updates | `skip = ["Safari*"]` |

Names match the way update-all shows them, in any case. A trailing `*`
matches any ending, and `"*"` alone matches everything in a section. A rule
in one section never affects another, so `skip = ["node"]` in `[brew]` leaves
npm's node packages alone.

```toml
ignore_paths = ["/Applications/Games"]

[brew]
pin = { node = "22" }

[app]
skip = ["Slack"]

[docker]
skip = ["*"]
```

`ignore_paths` has to come before the first section, because TOML puts every
key after a section header into that section. update-all rejects any other
key or section with an error, so a typo can't go unnoticed.

## Safety rules

- update-all never installs anything that isn't already installed.
- Each tool updates through whatever installed it. update-all follows each
  binary's symlinks to find its owner, so no tool gets two updaters.
- Apps that Homebrew doesn't manage update through their vendor's own
  updater. update-all uses Homebrew's public catalog only to look up versions,
  and it never moves an app into Homebrew.
- For a cask whose app updates itself, update-all compares the app's real
  version, so it skips apps that already updated themselves.
- If Homebrew lists a cask whose app you deleted, update-all leaves it alone
  instead of installing the app again.
- A Sparkle update needs a valid EdDSA signature that matches the key inside
  the installed app. The new app must also pass `codesign` and come from the
  same developer team as the installed one.
- Open apps aren't replaced. Their updates wait until you quit them.
- Docker pulls happen only for tags whose registry digest changed, and
  containers keep running.
- Helm upgrades never run. update-all reads each chart repository's index
  directly, so it doesn't change Helm's own repository cache either.
- `--yes` never approves a restart.

## Files

| Path | Contents |
|---|---|
| `~/.config/update-all/config.toml` | Your optional config |
| `~/Library/Application Support/update-all/state.json` | Your shell screen answers and any pending restart |
| `~/Library/Caches/update-all/` | Downloaded catalogs and feeds |
| `~/Library/Logs/update-all/` | A log of every command, one file per run |

## Development

The `Justfile` holds every common task:

| Recipe | What it does |
|---|---|
| `just` | Lists every recipe |
| `just help` | Prints update-all's full help |
| `just run <flags>` | Runs update-all with any flags. With no flags, it prints help. |
| `just dry-run` | Previews updates without installing anything |
| `just check` | Runs formatting, clippy, tests, and Vale |
| `just fmt`, `just test`, `just vale` | Run one check at a time |
| `just format` | Formats the code in place |
| `just log` | Shows the newest run log |
| `just release` | Builds the optimized binary |
| `just install` | Installs the binary into `~/.cargo/bin` |
| `just clean` | Removes build output |

All prose follows the Google developer documentation style guide, checked
with [Vale](https://vale.sh). Vale checks:

- Every Markdown file and every code comment.
- Every message, label, and error in `src/copy.rs`, which holds all of the
  tool's user-facing text.
- The app and tool descriptions in the built-in registries.
- The comments in `config.toml`, `Justfile`, `.gitignore`, and `.vale.ini`,
  and the `Cargo.toml` description.

A test exports the strings and comments to `target/vale/` and fails on any
Vale finding, even a suggestion. `just vale` checks everything the same way.
Run `vale sync` once to download the Google style.

## Releases

Pushing a tag such as `v0.0.2` runs `.github/workflows/release.yml`. The
workflow checks that the tag matches the version in `Cargo.toml`, runs the
tests, builds the binary, and publishes the zip and its checksum as a GitHub
release. To rebuild an existing release, run the workflow by hand with that
tag.

The workflow signs and notarizes the binary only when the repo has these
secrets. Without them, the release still ships, and its notes explain how to
run an unsigned binary.

| Secret | Purpose |
|---|---|
| `APPLE_CERTIFICATE_BASE64` | The Developer ID signing certificate, as a base64 `.p12` file |
| `APPLE_CERTIFICATE_PASSWORD` | The password for that `.p12` file |
| `APPLE_TEAM_NAME` | The team name in the certificate's name |
| `APPLE_TEAM_ID` | The Apple developer team ID |
| `APPLE_ID` | The Apple Account that submits builds for notarization |
| `APPLE_APP_PASSWORD` | An app-specific password for that Apple Account |

## License

update-all is free software under the GNU General Public License v3.0. See
[LICENSE](LICENSE).

## Credits

The App Store support follows the approach of
[mas](https://github.com/mas-cli/mas), which is Copyright 2015 Andrew Naylor
and Ross Goldberg and available under the MIT License.
