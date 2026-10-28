//! Every sentence update-all shows to people lives here, so Vale can check
//! all of it against the Google developer documentation style guide. A test
//! exports these strings to target/vale/copy.md for `vale` to read.
//!
//! `{}` marks a value that [`fill`] inserts at run time.

/// Replaces each `{}` in `template` with the next value from `values`.
pub fn fill(template: &str, values: &[&str]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    let mut values = values.iter();
    while let Some(idx) = rest.find("{}") {
        out.push_str(&rest[..idx]);
        out.push_str(values.next().copied().unwrap_or(""));
        rest = &rest[idx + 2..];
    }
    out.push_str(rest);
    out
}

macro_rules! copy {
    ($($name:ident = $text:expr;)*) => {
        $(pub const $name: &str = $text;)*
        /// Every string in this module, by name.
        #[cfg(test)]
        pub const ALL: &[(&str, &str)] = &[$((stringify!($name), $text)),*];
    };
}

pub const TUI_COLUMNS: [&str; 9] = [
    "",
    "Name",
    "Path",
    "What it does",
    "Current",
    "Released",
    "New",
    "Released",
    "Days",
];
pub const SHELL_COLUMNS: [&str; 3] = ["", "Framework", "Folder"];

copy! {
    // ---------- help ----------
    ABOUT = "Checks everything on this Mac for updates and updates what you choose.";
    SHORT_FOOTER = "Run `update-all --help` for the full guide: every source, the config file, and the safety rules.";
    LONG_HELP = "\
What update-all checks:
  Homebrew                Formulae and casks. A cask for an app that updates itself counts
                          only when the app on disk is older than the newest cask.
  Package managers        Global npm, pipx, uv, Cargo, Bun, and gem packages, rustup
                          toolchains, and standalone uv, Bun, and Deno. pip packages are
                          listed but never changed.
  AI command-line tools   Claude Code, Antigravity, opencode, and other tools that update
                          themselves. Tools from Homebrew or npm update with those.
  Apps and browsers       Browsers, AI desktop apps, and developer apps. With
                          --update-literally-all, every other app in /Applications and
                          ~/Applications too.
  App Store               Opt-in. Apps from the Mac App Store, including Xcode.
  Developer tools         Extensions for VS Code and editors built on it, gh extensions,
                          and Google Cloud CLI components.
  Docker images           Tags that now point to a newer image in the registry.
                          update-all pulls them and never restarts containers.
  Helm charts             Report only. Releases with newer charts, each with the command
                          that upgrades it. Hidden when helm isn't installed.
  Shell frameworks        Opt-in on the first-run screen. Covers Oh My Zsh, Prezto,
                          zinit, antidote, Fisher, tpm, and others.
  macOS                   macOS, Safari, and Command Line Tools from softwareupdate.

How a run works:
  1. update-all checks every source at once. A source whose tool isn't installed stays silent.
  2. It lists what's out of date, grouped by source.
  3. You answer yes, no, or pick. Pick opens a full-screen list where you choose each item.
  4. Updates run in order, with Homebrew first.
  5. Updates that need a restart run last, and only after you confirm.

Opt-in groups:
  Electron apps and App Store apps stay out of a run unless you include them. update-all
  asks about each group before the main question, or you can pass --electron or
  --app-store. --electron-only and --app-store-only check only those groups.

Days behind:
  The Days column shows how many days apart the installed and newest releases are. A value
  like ~12 means update-all knows only the newest release date, so it counts the days since
  that release instead. Yellow means 7 days or more, orange 14 or more, and red 30 or more.

Picker keys:
  Space toggles an item, a selects all, n selects none, / filters, Enter starts the
  updates, and q quits without changes.

Safety rules:
  - update-all never installs anything that isn't already installed.
  - Each tool updates through whatever installed it, so no tool gets two updaters.
  - Apps that Homebrew doesn't manage update through their vendor's own updater.
    update-all never moves them into Homebrew.
  - A Sparkle update needs a valid signature from the app's developer, and the new app must
    come from the same developer as the installed one.
  - Open apps aren't replaced. Their updates wait until you quit them.
  - --dry-run changes no installed software. It still refreshes package lists, such as
    Homebrew's, so the preview is accurate.
  - --yes never approves a restart.

Configuration:
  update-all reads ~/.config/update-all/config.toml at startup, or the file you pass with
  --config. Ideally the file is empty or missing. The file is TOML. It can hold
  ignore_paths, a list of folders update-all never looks in, and one section for each kind
  of item, such as [brew], [npm], [app], or [docker]. Each section accepts two settings:
    skip           Items update-all never checks.
    pin            Items to hold at a version or version prefix.
  Names match the way update-all shows them, in any case. A trailing * matches any ending,
  so skip = [\"*\"] in [docker] skips every Docker image. The example config.toml in the
  repo shows every section.

Files:
  ~/Library/Logs/update-all/                       A log of every command, one file per run.
  ~/Library/Caches/update-all/                     Downloaded catalogs and feeds.
  ~/Library/Application Support/update-all/        Your shell screen answers.";

    ARG_DRY_RUN = "Show what would update and the commands that would run, then stop";
    ARG_YES = "Answer yes to every question except the restart question";
    ARG_LITERALLY_ALL = "Also check every other app in /Applications and ~/Applications";
    ARG_ELECTRON = "Include Electron apps. By default, update-all leaves them out";
    ARG_ELECTRON_ONLY = "Check and update only Electron apps";
    ARG_APP_STORE = "Include App Store apps. By default, update-all leaves them out";
    ARG_APP_STORE_ONLY = "Check and update only App Store apps";
    ARG_CONFIGURE_SHELLS = "Choose again which shell frameworks to update";
    ARG_CONFIG = "Read this config file instead of ~/.config/update-all/config.toml";
    ARG_HELP = "Show help. --help shows the full guide";
    ARG_VERSION = "Show the version";

    // ---------- run flow ----------
    ERR_NOT_MACOS = "update-all runs only on macOS.";
    ERR_CONFIG = "update-all can't read its config file: {}";
    CONTINUING_AFTER_RESTART = "Welcome back. The last run installed updates that needed a restart: {}. Checking for anything that came after them.";
    CHECKING = "Checking for updates";
    SPINNER_CHECKING = "checking";
    SPINNER_NONE = "up to date";
    SPINNER_ONE = "1 update";
    SPINNER_MANY = "{} updates";
    ALL_CURRENT = "Everything is up to date.";
    ALL_CURRENT_PARTIAL = "update-all found no updates, but some checks didn't finish. The notes explain which ones.";
    HEADING_HELD = "Held back by config.toml";
    HEADING_NOTES = "Notes";
    HEADING_DRY_RUN = "Commands a real run would use";
    DRY_RUN_DONE = "This was a dry run, so update-all changed nothing.";
    TAG_OPT_IN = "opt-in, left out unless included";
    TAG_RESTART = "needs a restart";
    TAG_RESTART_LAST = "runs last, needs a restart";
    DESCRIBE_SPARKLE = "download and verify {}, then replace the app";
    DESCRIBE_APP_STORE = "update App Store item {}";
    LOG_AT = "Log: {}";
    NOT_INTERACTIVE = "update-all needs a terminal to ask what to update. Pass --yes to update everything without asking.";
    ASK_ELECTRON = "Include {} Electron apps?";
    ASK_APP_STORE = "Include {} App Store apps?";
    ASK_MAIN = "Update {} items? [y]es, [n]o, or [p]ick:";
    ASK_MAIN_HELP = "Type y to update everything listed, n to stop, or p to choose items.";
    NOTHING_CHANGED = "update-all changed nothing.";
    ASK_SUDO = "Some updates need administrator rights. Enter your Mac password if asked.";
    HEADING_RESULTS = "Results";
    REQUESTED = "the app's updater is running. Relaunch the app to finish";
    HEADING_RESTART = "These updates need a restart";
    RESTART_RERUN = "After the restart, run update-all again. Some updates appear only after you install others.";
    ASK_RESTART = "Install them now and restart this Mac?";
    RESTART_SKIPPED = "Skipped. Run update-all again when you're ready to restart.";
    RESTARTING = "Restarting this Mac.";
    RESTARTING_AFTER = "Running `{}`. The Mac restarts when the install finishes.";

    // ---------- results ----------
    ERR_STILL_OUTDATED = "Homebrew still lists it as outdated. Check the Homebrew output for the reason";
    ERR_COMMAND_FAILED = "the update command failed. Check its output for the reason";
    ERR_NO_SUDO = "needs administrator rights, which weren't granted";
    ERR_APP_RUNNING = "the app is open";
    ERR_NO_SIGNATURE = "the update has no signature to check, so update-all won't install it";
    ERR_DOCKER_RATE_LIMITED = "the registry's rate limit refused the pull. Run `docker login` to raise the limit, or try again in a few hours";
    SPARKLE_DOWNLOADING = "Downloading {}";
    SPARKLE_INSTALLED = "Checked the signature and the developer team, then installed version {}.";
    QUEUED_QUIT_APP = "quit the app, then run update-all again";

    // ---------- picker ----------
    TUI_TITLE = "Choose updates";
    TUI_COUNT = "{} of {} selected";
    TUI_FILTER = "Filter";
    TUI_KEYS = "Space: toggle   a: all   n: none   /: filter   Enter: update   q: quit";
    TUI_NO_MATCH = "No items match the filter.";
    TUI_NEEDS_RESTART = "Needs a restart, so it runs last and asks first.";

    // ---------- shell screen ----------
    SHELL_TITLE = "Shell frameworks";
    SHELL_INTRO = "update-all found these shell frameworks and plugin managers. They're off by default. Turn on the ones you want updated on every run. Run update-all --configure-shells to change this later.";
    SHELL_KEYS = "Space: toggle   Enter: save   Esc: decide later";
    SHELL_RUNS = "Update command: {}";
    SHELL_NONE_FOUND = "update-all didn't find any shell frameworks to configure.";
    DESC_SHELL = "{} and the plugins it manages";
    VERSION_UNCHECKED = "not checked";
    VERSION_COMMITS = "{} +{} commits";
    NOTE_SHELL_UNCHECKED = "This framework has no way to check for updates first, so update-all runs its update every time.";
    WARN_ZINIT_LEGACY = "This zinit checkout points at an old zdharma repository that's gone. Move to zdharma-continuum/zinit to keep getting updates.";
    WARN_BLESH = "ble.sh builds from source and needs GNU make and gawk.";
    WARN_FISHER = "fisher update removes plugins that aren't listed in fish_plugins.";

    // ---------- notes from sources ----------
    NOTE_PINNED = "pinned to {} in config.toml";
    NOTE_BREW_PINNED = "pinned in Homebrew with `brew pin`";
    NOTE_BREW_UPDATE_FAILED = "`brew update` failed, so Homebrew's package lists may be old.";
    NOTE_BREW_OUTDATED_FAILED = "`brew outdated` failed, so Homebrew packages weren't checked.";
    NOTE_CASK_APP_MISSING = "Homebrew lists the cask {}, but {} isn't installed anymore, so update-all leaves it alone. To clean up Homebrew's record, run `brew uninstall --cask` with that name.";
    NOTE_CASK_SELF_UPDATES = "updates itself too";
    NOTE_RUSTUP_AND_BREW_RUST = "This Mac has both rustup and Homebrew's rust. The rustc on your PATH comes from Homebrew, so rustup's toolchains apply only where ~/.cargo/bin comes first in PATH.";
    NOTE_HELM_NO_CLUSTER = "Helm couldn't reach the Kubernetes cluster in your current context, so Helm releases weren't checked.";
    NOTE_HELM_UNKNOWN_CHARTS = "These Helm charts aren't in any repository you added, so update-all couldn't check them: {}.";
    REPORT_HELM_TITLE = "Helm releases with newer charts, for you to review";
    REPORT_HELM_WHY = "update-all never runs these. Review each chart's release notes, then run the commands yourself.";
    REPORT_HELM_LINE = "{} in namespace {}: {} {} from {} -> {} from {}, {} days";
    REPORT_PIP_TITLE = "Report only: outdated {} packages";
    REPORT_PIP_WHY = "These belong to Homebrew's Python. Upgrading them with pip can break Homebrew, so update-all only lists them. Use a virtual environment, pipx, or uv for tools you manage yourself.";

    REPORT_MANUAL_TITLE = "Apps to update yourself";
    REPORT_MANUAL_WHY = "These apps have newer versions, but no update channel that update-all can drive. Open each app and use its own update menu.";
    REPORT_MANUAL_LINE = "{}  {} -> {}";
    NOTE_LAUNCH_HIDDEN = "opens in the background so its own updater runs";
    NOTE_DOCKER_DOWN = "Docker isn't running, so update-all didn't check Docker images.";
    NOTE_DOCKER_RATE_LIMIT = "A registry's rate limit stopped update-all from checking {} images. Run `docker login` to raise the limit, or try again in a few hours.";
    NOTE_DOCKER_FAILED = "update-all couldn't reach the registry for {} images, so it didn't check them.";
    NOTE_DOCKER_NO_BUILDX = "Docker images weren't checked because `docker buildx` is missing, and update-all needs it to read registry digests without pulling.";
    ERR_APP_STORE_OPENED = "opened the App Store's Updates page. Finish the update there";

    // ---------- labels ----------
    LABEL_BREW = "Homebrew";
    LABEL_LANGS = "Package managers";
    LABEL_AI = "AI command-line tools";
    LABEL_APPS = "Apps";
    LABEL_APPS_AND_BROWSERS = "Apps and browsers";
    LABEL_APP_STORE = "App Store";
    LABEL_DEVTOOLS = "Developer tools";
    LABEL_DOCKER = "Docker images";
    LABEL_HELM = "Helm charts";
    LABEL_SHELLS = "Shell frameworks";
    LABEL_MACOS = "macOS";
    STATUS_UPDATED = "updated";
    STATUS_REQUESTED = "requested";
    STATUS_QUEUED = "queued";
    STATUS_FAILED = "failed";
    TAG_ELECTRON = "Electron";
    SHELL_NAME_OMZ_CUSTOM = "Oh My Zsh custom plugins and themes";
    SHELL_NAME_ZSH_PLUGINS = "Standalone zsh plugins";
    SHELL_NAME_TPM = "tmux plugins managed by tpm";
    LOG_TIMED_OUT = ", timed out";
    LOG_NOT_FOUND = "not found";

    // ---------- errors from updates ----------
    ERR_HTTP = "HTTP {} from {}";
    ERR_SPARKLE_KEY = "the app's public key isn't 32 bytes long";
    ERR_SPARKLE_SIG_SIZE = "the update's signature isn't 64 bytes long";
    ERR_SPARKLE_SIG = "the update's signature doesn't match the app's public key";
    ERR_SPARKLE_SIZE = "the download is {} bytes, but the feed says {}";
    ERR_SPARKLE_NO_APP = "the download has no app with the ID {}";
    ERR_SPARKLE_CODESIGN = "the new app's code signature is invalid: {}";
    ERR_SPARKLE_TEAM = "the new app comes from developer team {}, but the installed app comes from {}";
    TEAM_NONE = "none";
    ERR_SPARKLE_MOVE = "update-all couldn't move {}. Replacing it may need administrator rights";
    ERR_SPARKLE_COPY = "update-all couldn't copy the new app: {}";
    ERR_SPARKLE_UNZIP = "update-all couldn't unzip the update: {}";
    ERR_SPARKLE_DMG = "update-all couldn't open the disk image: {}";
    ERR_SPARKLE_DMG_EMPTY = "the disk image has no app";
    ERR_SPARKLE_UNPACK = "update-all couldn't unpack the update: {}";
    ERR_SPARKLE_KIND = "update-all can't install this kind of update: {}";
    ERR_STORE_FAILED = "the App Store reported a failure";
    ERR_STORE_CANCELLED = "someone cancelled the download";
    ERR_STORE_MISSING = "the App Store framework has no {} class";
    ERR_STORE_UNAVAILABLE = "the App Store frameworks aren't available";
    ERR_STORE_EXCEPTION = "the App Store framework raised an exception: {}";
    ERR_STORE_LOST = "update-all lost track of the download";
    ERR_STORE_UNFINISHED = "the App Store didn't finish the update";
    ERR_STORE_QUEUE = "the App Store download queue isn't available";
    ERR_STORE_REQUEST = "update-all couldn't create the App Store request";
    ERR_STORE_PURCHASE = "the App Store purchase service isn't available";
    ERR_STORE_NOTHING = "the App Store returned nothing to download";
    ERR_STORE_GONE = "the App Store's installer failed, and its download was already gone";
    ERR_STORE_INSTALLER = "installer couldn't install the App Store package";

    // ---------- descriptions ----------
    DESC_DOCKER = "Container image whose tag now points to a newer image";
    DESC_RUSTUP = "Rust toolchain managed by rustup";
    DESC_UV = "Python package and project manager";
    DESC_BUN = "JavaScript runtime, bundler, and package manager";
    DESC_DENO = "JavaScript and TypeScript runtime";
    DESC_SOFTWAREUPDATE = "Apple system update";
    DESC_GH_EXTENSION = "GitHub CLI extension";
    DESC_GCLOUD = "Google Cloud CLI component";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_replaces_in_order() {
        assert_eq!(fill("{} of {}", &["1", "2"]), "1 of 2");
        assert_eq!(fill("no values", &[]), "no values");
        assert_eq!(fill("{} and {}", &["a"]), "a and ");
    }

    /// Writes every user-facing string to target/vale/ and runs Vale on it
    /// if this Mac has Vale. `just vale` checks the same files.
    #[test]
    fn copy_passes_vale() {
        use std::fmt::Write as _;
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/vale");
        std::fs::create_dir_all(&dir).unwrap();
        let mut md = String::from("# Strings\n\n");
        let mut all: Vec<(String, String)> = ALL
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        all.extend(
            crate::sources::described()
                .into_iter()
                .map(|(k, v)| (k, v.to_string())),
        );
        for (name, text) in all {
            let text = text.replace("{}", "3");
            let _ = writeln!(md, "`{name}`\n");
            if text.contains('\n') {
                let _ = writeln!(
                    md,
                    "{}\n",
                    text.lines().map(str::trim).collect::<Vec<_>>().join("\n\n")
                );
            } else {
                let _ = writeln!(md, "{text}\n");
            }
        }
        std::fs::write(dir.join("copy.md"), &md).unwrap();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let read = |name: &str| std::fs::read_to_string(root.join(name)).unwrap_or_default();
        let mut files = String::from("# Other files\n\n");
        for name in [
            "config.toml",
            "Justfile",
            ".gitignore",
            ".vale.ini",
            ".github/workflows/release.yml",
        ] {
            let _ = writeln!(files, "`{name}`\n\n{}\n", prose(&read(name)));
        }
        if let Some(desc) = read("Cargo.toml")
            .lines()
            .find_map(|l| l.strip_prefix("description = "))
        {
            let _ = writeln!(files, "`Cargo.toml`\n\n{}\n", desc.trim_matches('"'));
        }
        std::fs::write(dir.join("files.md"), files).unwrap();
        let _ = std::fs::remove_file(dir.join("config.md"));

        let Ok(out) = std::process::Command::new("vale")
            .args(["--no-exit", "--output=line", "--minAlertLevel=suggestion"])
            .arg(&dir)
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
        else {
            return;
        };
        let report = String::from_utf8_lossy(&out.stdout);
        assert!(report.trim().is_empty(), "Vale found problems:\n{report}");
    }

    /// Turns `#` comment lines into Markdown paragraphs for Vale. Commented-out
    /// YAML and indented examples are code, so the export leaves them out.
    fn prose(text: &str) -> String {
        fn flush(paragraphs: &mut Vec<String>, current: &mut Vec<String>) {
            if !current.is_empty() {
                paragraphs.push(current.join(" "));
                current.clear();
            }
        }
        let mut paragraphs: Vec<String> = Vec::new();
        let mut current: Vec<String> = Vec::new();
        for line in text.lines() {
            // Workflow step names and input descriptions show in the GitHub UI.
            let shown = ["name: ", "- name: ", "description: "]
                .iter()
                .find_map(|key| line.trim_start().strip_prefix(key));
            if let Some(value) = shown {
                flush(&mut paragraphs, &mut current);
                paragraphs.push(codeify(value.trim().trim_matches('\'')));
                continue;
            }
            let Some(c) = line.trim_start().strip_prefix('#') else {
                flush(&mut paragraphs, &mut current);
                continue;
            };
            let c = c.strip_prefix(' ').unwrap_or(c);
            // Linters read their own directives from comments, so skip those.
            if c.starts_with("shellcheck ") {
                continue;
            }
            let yaml_key = c.trim_end().ends_with(':') && !c.trim().contains(' ');
            let toml = c.starts_with(['[', ']', '"']) || c.contains(" = ");
            if c.trim().is_empty() || c.starts_with(' ') || c.starts_with('-') || yaml_key || toml {
                flush(&mut paragraphs, &mut current);
                // A commented-out setting can carry its own trailing comment.
                if let Some((_, note)) = c.rsplit_once(" # ") {
                    paragraphs.push(codeify(note.trim()));
                }
                continue;
            }
            current.push(codeify(c));
        }
        flush(&mut paragraphs, &mut current);
        paragraphs.join("\n\n")
    }

    /// Wraps paths, flags, and patterns in backticks so Vale reads them as code.
    fn codeify(line: &str) -> String {
        line.split('`')
            .enumerate()
            .map(|(i, part)| {
                if i % 2 == 1 {
                    return format!("`{part}`");
                }
                part.split(' ')
                    .map(|word| {
                        let core = word.trim_end_matches(['.', ',', ';', ':', ')']);
                        let tail = &word[core.len()..];
                        let inner_dot = core.trim_start_matches('.').contains('.');
                        let code = core.contains(['/', ':', '*', '~', '$'])
                            || core.starts_with("--")
                            || core.starts_with('.')
                            || inner_dot;
                        if code && !core.is_empty() {
                            format!("`{core}`{tail}")
                        } else {
                            word.to_string()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect()
    }
}
