//! Shell frameworks and plugin managers for zsh, bash, fish, and tmux. They
//! update only when the user opts in on the first-run screen.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{Ctx, par_map};
use crate::copy::{self, fill};
use crate::dates;
use crate::exec;
use crate::model::{Action, Discovery, Item, Release, Source};

/// A framework found on disk.
pub struct Found {
    pub id: &'static str,
    pub name: &'static str,
    pub root: PathBuf,
    /// Git checkouts to compare with their upstream.
    checkouts: Vec<PathBuf>,
    /// The update, run with `/bin/sh -c`.
    script: String,
    /// False for frameworks with no read-only way to check, like fisher.
    checkable: bool,
    warning: Option<&'static str>,
}

fn home() -> PathBuf {
    exec::home()
}

fn env_dir(var: &str, default: impl FnOnce() -> PathBuf) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(default)
}

fn zdotdir() -> PathBuf {
    env_dir("ZDOTDIR", home)
}

fn q(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

fn under_brew(path: &Path) -> bool {
    let real = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    real.starts_with("/opt/homebrew") || real.starts_with("/usr/local/Cellar")
}

/// Folders holding a `.git`, from `base` down to `depth` levels.
fn git_dirs(base: &Path, depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![(base.to_path_buf(), 0)];
    while let Some((dir, d)) = stack.pop() {
        if dir.join(".git").exists() {
            out.push(dir);
            continue;
        }
        if d >= depth {
            continue;
        }
        if let Ok(entries) = fs::read_dir(&dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir()
                    && !p
                        .file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with('.'))
                {
                    stack.push((p, d + 1));
                }
            }
        }
    }
    out.sort();
    out
}

fn pulls(dirs: &[PathBuf]) -> String {
    dirs.iter()
        .map(|d| format!("git -C {} pull --ff-only --quiet", q(d)))
        .collect::<Vec<_>>()
        .join(" && ")
}

/// Finds the line that sources `file_name` in a shell startup file.
fn sourced(rc: &Path, file_name: &str) -> Option<PathBuf> {
    let text = fs::read_to_string(rc).ok()?;
    text.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .find_map(|l| {
            let idx = l.find(file_name)?;
            let start = l[..idx].rfind([' ', '"', '\'', '='])? + 1;
            let raw = l[start..idx + file_name.len()]
                .replace("${HOME}", "~")
                .replace("$HOME", "~");
            let path = exec::expand(&raw);
            path.exists().then_some(path)
        })
}

fn zshrc() -> PathBuf {
    zdotdir().join(".zshrc")
}

macro_rules! found {
    ($id:expr, $name:expr, $root:expr, $checkouts:expr, $script:expr) => {
        Found {
            id: $id,
            name: $name,
            root: $root,
            checkouts: $checkouts,
            script: $script,
            checkable: true,
            warning: None,
        }
    };
}

fn oh_my_zsh() -> Option<Found> {
    let d = env_dir("ZSH", || home().join(".oh-my-zsh"));
    let upgrade = d.join("tools/upgrade.sh");
    upgrade.exists().then(|| {
        found!(
            "oh-my-zsh",
            "Oh My Zsh",
            d.clone(),
            vec![d.clone()],
            // Runs the upgrade script directly, which skips `omz update`'s `exec zsh`.
            format!("ZSH={} zsh -f {} -v minimal", q(&d), q(&upgrade))
        )
    })
}

fn oh_my_zsh_custom() -> Option<Found> {
    let omz = env_dir("ZSH", || home().join(".oh-my-zsh"));
    let custom = env_dir("ZSH_CUSTOM", || omz.join("custom"));
    let mut dirs = git_dirs(&custom.join("plugins"), 1);
    dirs.extend(git_dirs(&custom.join("themes"), 1));
    (!dirs.is_empty()).then(|| {
        found!(
            "oh-my-zsh-custom",
            copy::SHELL_NAME_OMZ_CUSTOM,
            custom.clone(),
            dirs.clone(),
            pulls(&dirs)
        )
    })
}

fn prezto() -> Option<Found> {
    let d = zdotdir().join(".zprezto");
    d.join("init.zsh").exists().then(|| {
        found!(
            "prezto",
            "Prezto",
            d.clone(),
            vec![d.clone()],
            format!(
                "git -C {0} pull --ff-only --quiet && git -C {0} submodule sync --recursive --quiet && git -C {0} submodule update --init --recursive --quiet",
                q(&d)
            )
        )
    })
}

fn zinit() -> Option<Found> {
    let data = env_dir("XDG_DATA_HOME", || home().join(".local/share"));
    let candidates = [
        (data.join("zinit/zinit.git"), data.join("zinit")),
        (home().join(".zinit/bin"), home().join(".zinit")),
        (home().join(".zplugin/bin"), home().join(".zplugin")),
    ];
    let (bin, base) = candidates
        .into_iter()
        .find(|(b, _)| b.join("zinit.zsh").exists())?;
    let remote = exec::run(
        "git",
        &[
            "-C",
            &bin.display().to_string(),
            "remote",
            "get-url",
            "origin",
        ],
        Duration::from_secs(5),
    )
    .stdout;
    let mut checkouts = vec![bin.clone()];
    checkouts.extend(git_dirs(&base.join("plugins"), 1));
    let mut f = found!(
        "zinit",
        "zinit",
        base,
        checkouts,
        format!(
            "zsh -c 'source \"$1\"; zinit update --all -q -n' zinit {}",
            q(&bin.join("zinit.zsh"))
        )
    );
    // GitHub no longer has the old zdharma repos, or they hold unrelated code now.
    if remote.contains("zdharma/") && !remote.contains("zdharma-continuum") {
        f.warning = Some(copy::WARN_ZINIT_LEGACY);
        f.checkable = false;
    }
    Some(f)
}

fn antidote() -> Option<Found> {
    let script = sourced(&zshrc(), "antidote.zsh").or_else(|| {
        [
            home().join(".antidote/antidote.zsh"),
            PathBuf::from("/opt/homebrew/opt/antidote/share/antidote/antidote.zsh"),
        ]
        .into_iter()
        .find(|p| p.exists())
    })?;
    let bundles = env_dir("ANTIDOTE_HOME", || home().join("Library/Caches/antidote"));
    let mut checkouts = git_dirs(&bundles, 4);
    if !under_brew(&script)
        && let Some(dir) = script.parent()
        && dir.join(".git").exists()
    {
        checkouts.push(dir.to_path_buf());
    }
    Some(found!(
        "antidote",
        "antidote",
        bundles,
        checkouts,
        format!(
            "zsh -c 'source \"$1\"; antidote update' antidote {}",
            q(&script)
        )
    ))
}

fn antigen() -> Option<Found> {
    let d = env_dir("ADOTDIR", || home().join(".antigen"));
    let has_script = home().join("antigen.zsh").exists()
        || Path::new("/opt/homebrew/share/antigen/antigen.zsh").exists();
    (has_script && d.exists()).then(|| {
        found!(
            "antigen",
            "Antigen",
            d.clone(),
            git_dirs(&d.join("bundles"), 2),
            // Antigen only knows its bundles after .zshrc runs.
            "zsh -ic 'antigen update' </dev/null".to_string()
        )
    })
}

fn zplug() -> Option<Found> {
    let d = env_dir("ZPLUG_HOME", || home().join(".zplug"));
    d.join("init.zsh").exists().then(|| {
        found!(
            "zplug",
            "zplug",
            d.clone(),
            git_dirs(&d.join("repos"), 2),
            "zsh -ic 'zplug update' </dev/null".to_string()
        )
    })
}

fn zimfw() -> Option<Found> {
    let d = env_dir("ZIM_HOME", || zdotdir().join(".zim"));
    let script = [
        d.join("zimfw.zsh"),
        PathBuf::from("/opt/homebrew/opt/zimfw/share/zimfw.zsh"),
    ]
    .into_iter()
    .find(|p| p.exists())?;
    let mut cmd = format!("ZIM_HOME={} zsh {} update -q", q(&d), q(&script));
    if !under_brew(&script) {
        cmd.push_str(&format!(
            " && ZIM_HOME={} zsh {} upgrade -q",
            q(&d),
            q(&script)
        ));
    }
    Some(found!(
        "zimfw",
        "Zim",
        d.clone(),
        git_dirs(&d.join("modules"), 1),
        cmd
    ))
}

fn znap() -> Option<Found> {
    let script = sourced(&zshrc(), "znap.zsh")?;
    let dir = script.parent()?.to_path_buf();
    // `znap pull` pulls every repo next to znap, which may include unrelated
    // projects, so only znap's own checkout updates here.
    Some(found!(
        "znap",
        "Znap",
        dir.clone(),
        vec![dir.clone()],
        pulls(&[dir])
    ))
}

fn sheldon() -> Option<Found> {
    let config = env_dir("SHELDON_CONFIG_DIR", || home().join(".config/sheldon"));
    if !exec::has("sheldon") || !config.join("plugins.toml").exists() {
        return None;
    }
    let data = env_dir("SHELDON_DATA_DIR", || home().join(".local/share/sheldon"));
    Some(found!(
        "sheldon",
        "sheldon",
        config,
        git_dirs(&data.join("repos"), 4),
        "sheldon lock --update".to_string()
    ))
}

fn zgenom() -> Option<Found> {
    let d = home().join(".zgenom");
    let script = d.join("zgenom.zsh");
    if !script.exists() {
        return None;
    }
    let sources = if home().join(".zgen").exists() {
        home().join(".zgen")
    } else {
        d.join("sources")
    };
    let mut checkouts = vec![d.clone()];
    checkouts.extend(git_dirs(&sources, 2));
    Some(found!(
        "zgenom",
        "zgenom",
        d,
        checkouts,
        format!(
            "zsh -c 'source \"$1\"; zgenom update; zgenom selfupdate' zgenom {}",
            q(&script)
        )
    ))
}

fn zgen() -> Option<Found> {
    let d = home().join(".zgen");
    let script = d.join("zgen.zsh");
    if !script.exists() || home().join(".zgenom").exists() {
        return None;
    }
    Some(found!(
        "zgen",
        "zgen",
        d.clone(),
        git_dirs(&d, 2),
        format!(
            "zsh -c 'source \"$1\"; zgen update; zgen selfupdate' zgen {}",
            q(&script)
        )
    ))
}

fn zcomet() -> Option<Found> {
    let d = zdotdir().join(".zcomet");
    let script = d.join("bin/zcomet.zsh");
    if !script.exists() {
        return None;
    }
    let mut checkouts = vec![d.join("bin")];
    checkouts.extend(git_dirs(&d.join("repos"), 2));
    Some(found!(
        "zcomet",
        "zcomet",
        d,
        checkouts,
        format!(
            "zsh -c 'source \"$1\"; zcomet update; zcomet self-update' zcomet {}",
            q(&script)
        )
    ))
}

fn powerlevel10k() -> Option<Found> {
    let d = home().join("powerlevel10k");
    d.join(".git").exists().then(|| {
        found!(
            "powerlevel10k",
            "Powerlevel10k",
            d.clone(),
            vec![d.clone()],
            pulls(&[d])
        )
    })
}

fn zsh_plugins() -> Option<Found> {
    let mut dirs = Vec::new();
    for base in [
        home().join(".zsh"),
        home().join(".zsh/plugins"),
        home().join(".config/zsh/plugins"),
    ] {
        dirs.extend(git_dirs(&base, 1).into_iter().filter(|d| d != &base));
    }
    dirs.sort();
    dirs.dedup();
    (!dirs.is_empty()).then(|| {
        found!(
            "zsh-plugins",
            copy::SHELL_NAME_ZSH_PLUGINS,
            home().join(".zsh"),
            dirs.clone(),
            pulls(&dirs)
        )
    })
}

fn oh_my_bash() -> Option<Found> {
    let d = env_dir("OSH", || home().join(".oh-my-bash"));
    let upgrade = d.join("tools/upgrade.sh");
    upgrade.exists().then(|| {
        found!(
            "oh-my-bash",
            "Oh My Bash",
            d.clone(),
            vec![d.clone()],
            format!("OSH={} bash {}", q(&d), q(&upgrade))
        )
    })
}

fn bash_it() -> Option<Found> {
    let d = env_dir("BASH_IT", || home().join(".bash_it"));
    d.join("bash_it.sh").exists().then(|| {
        found!(
            "bash-it",
            "Bash-it",
            d.clone(),
            vec![d.clone()],
            // bash-it restarts the shell after updating; empty input ends it.
            "bash -ic 'bash-it update stable --silent' </dev/null".to_string()
        )
    })
}

fn ble_sh() -> Option<Found> {
    let script = home().join(".local/share/blesh/ble.sh");
    script.exists().then(|| Found {
        id: "ble.sh",
        name: "ble.sh",
        root: script.parent().map(Path::to_path_buf).unwrap_or_default(),
        checkouts: Vec::new(),
        script: format!("bash {} --update", q(&script)),
        checkable: false,
        warning: Some(copy::WARN_BLESH),
    })
}

fn fisher() -> Option<Found> {
    let f = home().join(".config/fish/functions/fisher.fish");
    (f.exists() && exec::has("fish")).then(|| Found {
        id: "fisher",
        name: "Fisher",
        root: home().join(".config/fish"),
        checkouts: Vec::new(),
        script: "fish -c 'fisher update'".to_string(),
        checkable: false,
        warning: Some(copy::WARN_FISHER),
    })
}

fn oh_my_fish() -> Option<Found> {
    let d = env_dir("OMF_PATH", || home().join(".local/share/omf"));
    (d.exists() && exec::has("fish")).then(|| {
        let mut checkouts = git_dirs(&d.join("pkg"), 1);
        checkouts.extend(git_dirs(&d.join("themes"), 1));
        found!(
            "oh-my-fish",
            "Oh My Fish",
            d.clone(),
            checkouts,
            "fish -c 'omf update'".to_string()
        )
    })
}

fn tpm() -> Option<Found> {
    let config = env_dir("XDG_CONFIG_HOME", || home().join(".config"));
    let d = [
        home().join(".tmux/plugins/tpm"),
        config.join("tmux/plugins/tpm"),
    ]
    .into_iter()
    .find(|p| p.join("bin/update_plugins").exists())?;
    let plugins = d.parent()?.to_path_buf();
    Some(found!(
        "tpm",
        copy::SHELL_NAME_TPM,
        plugins.clone(),
        git_dirs(&plugins, 1),
        format!("{} all", q(&d.join("bin/update_plugins")))
    ))
}

const DETECTORS: &[fn() -> Option<Found>] = &[
    oh_my_zsh,
    oh_my_zsh_custom,
    prezto,
    zinit,
    antidote,
    antigen,
    zplug,
    zimfw,
    znap,
    sheldon,
    zgenom,
    zgen,
    zcomet,
    powerlevel10k,
    zsh_plugins,
    oh_my_bash,
    bash_it,
    ble_sh,
    fisher,
    oh_my_fish,
    tpm,
];

/// Every framework on disk, for the first-run screen.
pub fn detect_all() -> Vec<Found> {
    DETECTORS.iter().filter_map(|d| d()).collect()
}

impl Found {
    pub fn update_command(&self) -> &str {
        &self.script
    }

    pub fn warning(&self) -> Option<&'static str> {
        self.warning
    }
}

struct Behind {
    commits: usize,
    head: String,
    head_date: Option<jiff::Timestamp>,
    upstream: String,
    upstream_date: Option<jiff::Timestamp>,
}

/// Fetches a checkout and counts the commits it's missing. Pinned checkouts
/// with no upstream branch count as current.
fn behind(dir: &Path) -> Option<Behind> {
    let d = dir.display().to_string();
    let env = [
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GIT_SSH_COMMAND", "ssh -o BatchMode=yes"),
    ];
    let git = |args: &[&str], secs: u64| {
        let mut all = vec!["-C", &d];
        all.extend_from_slice(args);
        exec::run_with("git", &all, &env, Duration::from_secs(secs))
    };
    git(&["fetch", "--quiet"], 60);
    let count = git(&["rev-list", "--count", "HEAD..@{u}"], 10);
    let commits: usize = count.ok().then(|| count.stdout.trim().parse().ok())??;
    if commits == 0 {
        return None;
    }
    let info = |rev: &str| {
        let out = git(&["log", "-1", "--format=%h %cI", rev], 10);
        let line = out.stdout.trim().to_string();
        let (hash, date) = line.split_once(' ').unwrap_or((&line, ""));
        (hash.to_string(), dates::parse(date))
    };
    let (head, head_date) = info("HEAD");
    let (upstream, upstream_date) = info("@{u}");
    Some(Behind {
        commits,
        head,
        head_date,
        upstream,
        upstream_date,
    })
}

pub fn discover(ctx: &Ctx) -> Discovery {
    let mut found = Discovery::default();
    if ctx.shells_enabled.is_empty() {
        return found;
    }
    let frameworks: Vec<Found> = detect_all()
        .into_iter()
        .filter(|f| ctx.shells_enabled.iter().any(|e| e == f.id))
        .collect();
    for f in frameworks {
        if let Some(w) = f.warning
            && !f.checkable
            && f.id == "zinit"
        {
            found.notes.push(w.to_string());
            continue;
        }
        let action = Action::Command {
            program: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), f.script.clone()],
            sudo: false,
        };
        let mut item = Item::new(Source::Shells, format!("shell:{}", f.id), f.name, action)
            .path(f.root.display().to_string())
            .description(fill(copy::DESC_SHELL, &[f.name]));
        if !f.checkable {
            item = item
                .versions(
                    Release::new("-", None),
                    Release::new(copy::VERSION_UNCHECKED, None),
                )
                .note(f.warning.unwrap_or(copy::NOTE_SHELL_UNCHECKED));
            found.items.push(item);
            continue;
        }
        let results: Vec<Behind> = par_map(f.checkouts.clone(), 8, |d| behind(&d))
            .into_iter()
            .flatten()
            .collect();
        if results.is_empty() {
            continue;
        }
        let commits: usize = results.iter().map(|b| b.commits).sum();
        let oldest = results.iter().filter_map(|b| b.head_date).min();
        let newest = results.iter().filter_map(|b| b.upstream_date).max();
        let (cur, new) = if results.len() == 1 {
            (results[0].head.clone(), results[0].upstream.clone())
        } else {
            ("-".to_string(), "-".to_string())
        };
        item = item.versions(
            Release::new(cur, oldest),
            Release::new(
                fill(copy::VERSION_COMMITS, &[&new, &commits.to_string()]),
                newest,
            ),
        );
        if let Some(w) = f.warning {
            item = item.note(w);
        }
        found.items.push(item);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_paths_for_sh() {
        assert_eq!(q(Path::new("/a b/c")), "'/a b/c'");
        assert_eq!(q(Path::new("/it's")), r"'/it'\''s'");
    }

    #[test]
    fn finds_sourced_files() {
        let dir = std::env::temp_dir().join(format!("update-all-test-{}", std::process::id()));
        fs::create_dir_all(dir.join("znap")).unwrap();
        fs::write(dir.join("znap/znap.zsh"), "").unwrap();
        let rc = dir.join(".zshrc");
        fs::write(
            &rc,
            format!(
                "# source nope/znap.zsh\nsource \"{}/znap/znap.zsh\"\n",
                dir.display()
            ),
        )
        .unwrap();
        assert_eq!(sourced(&rc, "znap.zsh"), Some(dir.join("znap/znap.zsh")));
        let _ = fs::remove_dir_all(dir);
    }
}
