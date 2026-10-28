# Agent guide

This guide is for coding agents that work on update-all. If this folder has
a `HANDOFF.md`, read it first. It holds local session notes that stay out of
Git: the current state, decisions, and open work.

## What this project is

update-all is a single-binary Rust command-line tool for Apple silicon Macs. It
checks everything installed on the Mac for updates, lists how far behind each
item is, and updates what the user picks. The [README](README.md) describes
every source and option from the user's side.

## Commands

| Command | Purpose |
|---|---|
| `just check` | Formatting, clippy with `-D warnings`, tests, and Vale. Run it before you finish any change. |
| `just dry-run` | Runs the tool on this Mac without changing installed software. |
| `just help` | Shows the full built-in guide. |
| `just run <flags>` | Runs the tool with any flags. With no flags, it prints help. |
| `just fmt` | Checks formatting. `just format` fixes it. |
| `just test` | Runs the tests only. |
| `just vale` | Checks every Markdown file, code comment, and user-facing string. |
| `just log` | Prints the newest run log from `~/Library/Logs/update-all/`. |

The `Justfile` lists every recipe, and `just` alone prints them.

Run `vale sync` once after cloning or moving the repo. Git ignores the
downloaded Google style, and only the project vocabulary is in the repo.

After moving the repo, also run `cargo clean`. Test binaries keep the old
folder path, so the Vale export test fails until they rebuild.

## Code map

| Path | Role |
|---|---|
| `src/main.rs` | Command-line flags, the run flow, the summary, and the final report |
| `src/copy.rs` | Every user-facing string, plus the test that exports them for Vale |
| `src/model.rs` | `Item`, `Action`, `Release`, `Discovery`, and the age colors |
| `src/apply.rs` | Runs selected updates in source order, then the restart step |
| `src/tui.rs` | The update picker and the first-run shell screen |
| `src/ui.rs` | Plain terminal output and prompts |
| `src/config.rs` | Loads `config.toml` |
| `src/state.rs` | Saves shell choices and the pending restart marker |
| `src/origin.rs` | Finds which installer owns a binary by following symlinks |
| `src/exec.rs` | Runs commands with timeouts, logging, and sudo |
| `src/http.rs` | HTTP client, ETag cache, and GitHub API access |
| `src/version.rs` | Loose version comparison and pin matching |
| `src/dates.rs` | Date parsing and GitHub release dates |
| `src/sources/` | One module per source, registered in `sources::CHECKS` |

## Rules that come from the user

These are requirements, not preferences. Don't weaken them.

- **Never install anything that isn't already installed.** A source whose
  tool is missing returns an empty `Discovery` and prints nothing.
- **Helm stays report-only.** Never run `helm upgrade` or `helm repo update`.
  Helm detection uses `exec::on_path`, so helm stays silent unless it's on the
  user's real `PATH`.
- **Electron apps and App Store apps are opt-in.** Only `--electron`,
  `--electron-only`, `--app-store`, `--app-store-only`, or a yes at the run's
  prompt includes them. `--yes` doesn't opt in.
- **Updates that need a restart run last.** The restart prompt always asks,
  even with `--yes`.
- **`--dry-run` changes no installed software.** It may still refresh package
  lists, for example with `brew update`.
- **Shell framework updates are off by default.** The first-run screen turns
  them on, and the choices live in `state.json`, never in `config.toml`.
- **`config.toml` holds only folders to ignore, items to skip, and items to
  pin.** It has `ignore_paths` plus one section per ID prefix, such as
  `[brew]` or `[app]`, and each section accepts only `skip` and `pin`.
  `[docker]` and `[shell]` accept only `skip`. Every struct uses
  `deny_unknown_fields`. The file's first line says it should ideally be empty
  or not exist.
- **No dependency on the `mas` binary.** App Store support ports the
  approach of mas into `src/sources/commerce.rs`.
- **Apps that Homebrew doesn't manage update through their vendor.** Use
  Homebrew's cask catalog only as a version source. Never adopt apps into
  Homebrew.
- **All prose follows the Google developer documentation style guide.**
  `just vale` must report 0 errors, 0 warnings, and 0 suggestions. That
  covers Markdown, code comments, `src/copy.rs`, registry descriptions, and
  the comments in `config.toml`, `Justfile`, `.gitignore`, and `.vale.ini`.
  Never put user-facing text in a string literal outside `src/copy.rs`.

## How to make common changes

**Add a user-facing string.** Add it to the `copy!` block in `src/copy.rs` and
use `fill` for `{}` placeholders. Then run `just vale`. Put code, commands, and
paths in backticks. Add product names that Vale flags to
`.vale/styles/config/vocabularies/UpdateAll/accept.txt`.

**Add a source.**
1. Create `src/sources/<name>.rs` with `pub fn discover(ctx: &Ctx) -> Discovery`.
1. Register it in `sources::CHECKS` and in the `Source` enum, whose order is
   the update order.
1. Use `origin::of` so a tool only updates through the installer that owns it.
1. If it needs a new kind of update, add an `Action` variant and handle it in
   `apply.rs` and in `describe` in `main.rs`.
1. Set `Discovery::incomplete` when a check fails, so the run doesn't claim
   everything is up to date.

**Add a known app or AI tool.** Add an entry to `KNOWN` in
`src/sources/apps.rs` or `TOOLS` in `src/sources/ai.rs`. Their descriptions
go through Vale too.

## Testing on the user's Mac

A real run updates real software. Use `just dry-run` freely. Before any real
update, ask the user, then limit the run with a temporary config that skips
everything else, for example:

```toml
[brew]
skip = ["*"]

[cask]
skip = ["*"]

[app]
skip = ["*"]
```

Pass it with `--config` and `--yes`, and compare `brew list --versions`, app
versions, and `docker images` before and after.

Many registries rate-limit anonymous requests. Docker Hub in particular blocks
manifest checks after a few full runs in a row.

## Commits

Every commit in this repo must show as verified on GitHub for the
swackhamer account. The repo's own Git config already sets this up:

| Setting | Value |
|---|---|
| `user.name` | `swackhamer` |
| `user.email` | `swackhamer@users.noreply.github.com` |
| `user.signingkey` | `~/.ssh/id_es25519_swackhamer.pub` |
| `gpg.format` | `ssh` |
| `commit.gpgsign` and `tag.gpgsign` | `true` |
| `core.sshCommand` | `ssh -i ~/.ssh/id_es25519_swackhamer -o IdentitiesOnly=yes` |

- Don't override the author or committer, and never pass `--no-gpg-sign`.
- The global Git config on this Mac uses a different identity, so never
  commit from a copy of the repo without these local settings.
- After each commit, run `git verify-commit HEAD`. It must print a good
  signature for `swackhamer@users.noreply.github.com`.
- Push over SSH, for example to `git@github.com:swackhamer/update-all.git`,
  so `core.sshCommand` authenticates with the swackhamer key.

## Releases

1. Change `version` in `Cargo.toml` and commit.
1. Tag the commit with a signed tag, for example
   `git tag -s v0.0.2 -m "update-all 0.0.2"`, then push the commit and the tag.
1. Watch the Build and Release workflow. It fails when the tag doesn't match
   `Cargo.toml`.

The README lists the Apple secrets the workflow needs to sign and notarize.
Release notes come from `.github/release-notes.md`, so Vale checks them too.

## Style

Match the surrounding code: short doc comments that explain why, `anyhow` for
errors in update paths, `Option` for lookups that may find nothing, and `par_map` for
parallel network checks. Keep comments in Google style too, because Vale
checks them.
