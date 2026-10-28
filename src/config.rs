//! Loads config.toml. The file holds `ignore_paths` and one section per kind
//! of item, such as `[brew]` or `[app]`. Each section accepts only `skip` and
//! `pin`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::exec;
use crate::model::Item;
use crate::version;

/// The file as written. Unknown keys and sections are errors, so a typo
/// can't go unnoticed.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct File {
    ignore_paths: Vec<String>,
    brew: Section,
    cask: Section,
    npm: Section,
    pipx: Section,
    uv: Section,
    cargo: Section,
    bun: Section,
    gem: Section,
    rustup: Section,
    #[serde(rename = "self")]
    self_updaters: Section,
    ai: Section,
    app: Section,
    appstore: Section,
    ext: Section,
    gh: Section,
    gcloud: Section,
    docker: SkipOnly,
    shell: SkipOnly,
    macos: Section,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Section {
    skip: Vec<String>,
    pin: BTreeMap<String, String>,
}

/// For items whose versions are digests or commits, where a pin means nothing.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SkipOnly {
    skip: Vec<String>,
}

/// One `skip` or `pin` entry, tied to the section it came from.
#[derive(Debug)]
struct Rule {
    prefix: &'static str,
    pattern: String,
}

#[derive(Debug, Default)]
pub struct Config {
    /// Folders never searched for apps or tools.
    pub ignore_paths: Vec<String>,
    ignore: Vec<PathBuf>,
    skip: Vec<Rule>,
    pin: Vec<(Rule, String)>,
}

/// The default location: $XDG_CONFIG_HOME/update-all/config.toml, or
/// ~/.config/update-all/config.toml.
pub fn default_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| exec::home().join(".config"))
        .join("update-all/config.toml")
}

impl Config {
    /// Reads the config. A missing or empty file gives the defaults. A path
    /// passed with --config must exist.
    pub fn load(explicit: Option<&Path>) -> Result<Config> {
        let path = explicit.map(Path::to_path_buf).unwrap_or_else(default_path);
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if explicit.is_none() && e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Config::default());
            }
            Err(e) => return Err(e).with_context(|| path.display().to_string()),
        };
        Config::parse(&text).with_context(|| path.display().to_string())
    }

    pub fn parse(text: &str) -> Result<Config> {
        let file: File = toml::from_str(text)?;
        let sections: [(&'static str, &Section); 17] = [
            ("brew", &file.brew),
            ("cask", &file.cask),
            ("npm", &file.npm),
            ("pipx", &file.pipx),
            ("uv", &file.uv),
            ("cargo", &file.cargo),
            ("bun", &file.bun),
            ("gem", &file.gem),
            ("rustup", &file.rustup),
            ("self", &file.self_updaters),
            ("ai", &file.ai),
            ("app", &file.app),
            ("appstore", &file.appstore),
            ("ext", &file.ext),
            ("gh", &file.gh),
            ("gcloud", &file.gcloud),
            ("macos", &file.macos),
        ];
        let rule = |prefix, pattern: &String| Rule {
            prefix,
            pattern: pattern.trim().to_lowercase(),
        };
        let mut config = Config {
            ignore: file.ignore_paths.iter().map(|p| exec::expand(p)).collect(),
            ..Config::default()
        };
        for (prefix, section) in sections {
            config
                .skip
                .extend(section.skip.iter().map(|p| rule(prefix, p)));
            config.pin.extend(
                section
                    .pin
                    .iter()
                    .map(|(p, v)| (rule(prefix, p), v.clone())),
            );
        }
        for (prefix, section) in [("docker", &file.docker), ("shell", &file.shell)] {
            config
                .skip
                .extend(section.skip.iter().map(|p| rule(prefix, p)));
        }
        config.ignore_paths = file.ignore_paths;
        Ok(config)
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.ignore_paths.is_empty() && self.pin.is_empty() && self.skip.is_empty()
    }

    /// True when `path` sits inside an ignored folder.
    pub fn ignored(&self, path: &Path) -> bool {
        self.ignore.iter().any(|dir| path.starts_with(dir))
    }

    pub fn skipped(&self, item: &Item) -> bool {
        self.skips(&item.id, &item.name)
    }

    /// True when a skip rule covers this ID or name. Sources call it before
    /// network checks, so skipped items cost no requests.
    pub fn skips(&self, id: &str, name: &str) -> bool {
        self.skip.iter().any(|r| r.matches(id, name))
    }

    /// The pin that applies to an item, if any.
    pub fn pin_for(&self, item: &Item) -> Option<&str> {
        self.pin
            .iter()
            .find(|(r, _)| r.matches(&item.id, &item.name))
            .map(|(_, v)| v.as_str())
    }

    /// Applies skips and pins. Pinned items whose new version doesn't match
    /// the pin stay visible, and update-all holds them back.
    pub fn apply(&self, items: Vec<Item>) -> Vec<Item> {
        items
            .into_iter()
            .filter(|item| !self.skipped(item) && !self.ignored(Path::new(&item.path)))
            .map(|mut item| {
                if let Some(pin) = self.pin_for(&item)
                    && !version::matches_pin(&item.latest.version, pin)
                {
                    item.held = true;
                    item.note = Some(crate::copy::fill(crate::copy::NOTE_PINNED, &[pin]));
                }
                item
            })
            .collect()
    }
}

impl Rule {
    /// A rule from `[brew]` covers only `brew:` items. Within its section, it
    /// matches the rest of the ID or the display name, ignoring case. A
    /// trailing `*` matches any ending.
    fn matches(&self, id: &str, name: &str) -> bool {
        let Some(rest) = id
            .strip_prefix(self.prefix)
            .and_then(|r| r.strip_prefix(':'))
        else {
            return false;
        };
        let (rest, name) = (rest.to_lowercase(), name.to_lowercase());
        match self.pattern.strip_suffix('*') {
            Some(start) => rest.starts_with(start) || name.starts_with(start),
            None => rest == self.pattern || name == self.pattern,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Action, Release, Source};

    fn item(id: &str, name: &str, latest: &str) -> Item {
        Item::new(
            Source::Brew,
            id,
            name,
            Action::BrewFormula(name.to_string()),
        )
        .versions(Release::new("1.0", None), Release::new(latest, None))
    }

    #[test]
    fn commented_example_is_empty() {
        let config = Config::parse(include_str!("../config.toml")).unwrap();
        assert!(config.is_empty());
    }

    /// Removes the comment signs from every setting in the example and checks
    /// that the result parses, so the example stays valid TOML for anyone who
    /// copies it.
    #[test]
    fn uncommented_example_parses() {
        let active: String = include_str!("../config.toml")
            .lines()
            .filter_map(|l| l.strip_prefix("# "))
            .filter(|l| {
                l.starts_with('[')
                    || l.starts_with("skip =")
                    || l.starts_with("pin =")
                    || l.starts_with("ignore_paths =")
            })
            .map(|l| format!("{l}\n"))
            .collect();
        let config = Config::parse(&active).unwrap();
        assert_eq!(config.ignore_paths.len(), 2);
        assert_eq!(config.skip.len(), 19);
        assert_eq!(config.pin.len(), 3);
        assert!(config.skipped(&item("docker:postgres:16", "postgres:16", "2")));
        assert!(config.skipped(&item("ext:code:ms-python.python", "Python", "2")));
        assert_eq!(
            config.pin_for(&item("brew:node", "node", "22.1.0")),
            Some("22")
        );
    }

    #[test]
    fn rejects_unknown_keys_and_sections() {
        assert!(Config::parse("colors = true\n").is_err());
        assert!(Config::parse("[brew]\ncolor = true\n").is_err());
        assert!(Config::parse("[homebrew]\nskip = [\"wget\"]\n").is_err());
        assert!(Config::parse("[docker]\npin = { \"postgres:16\" = \"16\" }\n").is_err());
    }

    #[test]
    fn skips_stay_inside_their_section() {
        let config =
            Config::parse("[brew]\nskip = [\"node\"]\n\n[app]\nskip = [\"Slack\"]\n").unwrap();
        assert!(config.skipped(&item("brew:node", "node", "2")));
        assert!(!config.skipped(&item("npm:node", "node", "2")));
        assert!(!config.skipped(&item("brew:nodenv", "nodenv", "2")));
        assert!(config.skipped(&item("app:Slack", "Slack", "2")));
    }

    #[test]
    fn matches_display_names_and_wildcards() {
        let config = Config::parse(
            "[docker]\nskip = [\"*\"]\n\n[ext]\nskip = [\"Python*\"]\n\n[macos]\nskip = [\"Safari*\"]\n",
        )
        .unwrap();
        assert!(config.skipped(&item("docker:redis:7", "redis:7", "2")));
        assert!(config.skipped(&item(
            "ext:code:ms-python.python",
            "Python (Visual Studio Code)",
            "2"
        )));
        assert!(config.skipped(&item("macos:Safari27.3-27.3", "Safari", "2")));
        assert!(!config.skipped(&item("macos:macOS 27.3-26C10", "macOS 27.3", "2")));
    }

    #[test]
    fn pins_hold_items_back() {
        let config = Config::parse("[brew]\npin = { node = \"26\" }\n").unwrap();
        let out = config.apply(vec![
            item("brew:node", "node", "27.0.0"),
            item("brew:jq", "jq", "2.0"),
        ]);
        assert!(out[0].held);
        assert!(!out[1].held);
        let out = config.apply(vec![item("brew:node", "node", "26.13.0")]);
        assert!(!out[0].held);
    }

    #[test]
    fn ignores_folders() {
        let config = Config::parse("ignore_paths = [\"/Applications/Games\"]\n").unwrap();
        assert!(config.ignored(Path::new("/Applications/Games/Chess.app")));
        assert!(!config.ignored(Path::new("/Applications/Gamesmith.app")));
    }
}
