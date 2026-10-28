//! macOS system updates from `softwareupdate`: macOS itself, Safari, and the
//! Command Line Tools. Updates that need a restart always run last.

use std::time::Duration;

use super::Ctx;
use crate::copy;
use crate::exec;
use crate::model::{Action, Discovery, Item, Release, Source};

#[derive(Debug, PartialEq)]
struct Label {
    label: String,
    title: String,
    version: String,
    restart: bool,
}

pub fn discover(_ctx: &Ctx) -> Discovery {
    let mut found = Discovery::default();
    if !exec::has("softwareupdate") {
        return found;
    }
    let out = exec::run("softwareupdate", &["--list"], Duration::from_secs(600));
    for label in parse(&out.stdout) {
        let current = installed_version(&label.title);
        let mut item = Item::new(
            Source::MacOs,
            format!("macos:{}", label.label),
            &label.title,
            Action::SoftwareUpdate {
                label: label.label.clone(),
            },
        )
        .path("/usr/sbin/softwareupdate")
        .description(copy::DESC_SOFTWAREUPDATE)
        .versions(
            Release::new(current, None),
            Release::new(&label.version, None),
        );
        item.needs_restart = label.restart;
        found.items.push(item);
    }
    found
}

fn parse(text: &str) -> Vec<Label> {
    let mut labels = Vec::new();
    let mut pending: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(label) = line.strip_prefix("* Label: ") {
            pending = Some(label.trim().to_string());
        } else if let Some(details) = line.strip_prefix("Title: ")
            && let Some(label) = pending.take()
        {
            let field = |key: &str| {
                details
                    .split(", ")
                    .find_map(|kv| kv.strip_prefix(key))
                    .map(|v| v.trim().trim_end_matches(',').to_string())
            };
            let title = details.split(", ").next().unwrap_or_default().to_string();
            labels.push(Label {
                label,
                title,
                version: field("Version: ").unwrap_or_default(),
                restart: field("Action: ").is_some_and(|a| a.eq_ignore_ascii_case("restart")),
            });
        }
    }
    labels
}

fn installed_version(title: &str) -> String {
    let quick = Duration::from_secs(10);
    if title.starts_with("macOS") {
        return exec::run("sw_vers", &["-productVersion"], quick)
            .stdout
            .trim()
            .to_string();
    }
    if title.starts_with("Command Line Tools") {
        let out = exec::run(
            "pkgutil",
            &["--pkg-info=com.apple.pkg.CLTools_Executables"],
            quick,
        );
        return out
            .stdout
            .lines()
            .find_map(|l| l.strip_prefix("version: "))
            .unwrap_or_default()
            .to_string();
    }
    if title.starts_with("Safari") {
        return crate::sources::apps::AppInfo::read(std::path::Path::new(
            "/Applications/Safari.app",
        ))
        .map(|a| a.version)
        .unwrap_or_default();
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_softwareupdate_list() {
        let text = "Software Update Tool\n\nFinding available software\n\
Software Update found the following new or updated software:\n\
* Label: Command Line Tools for Xcode-27.1\n\
\tTitle: Command Line Tools for Xcode, Version: 27.1, Size: 1037577KiB, Recommended: YES, \n\
* Label: macOS Example 27.3-26C10\n\
\tTitle: macOS Example 27.3, Version: 27.3, Size: 7088248KiB, Recommended: YES, Action: restart, \n";
        let labels = parse(text);
        assert_eq!(labels.len(), 2);
        assert_eq!(labels[0].label, "Command Line Tools for Xcode-27.1");
        assert_eq!(labels[0].title, "Command Line Tools for Xcode");
        assert_eq!(labels[0].version, "27.1");
        assert!(!labels[0].restart);
        assert!(labels[1].restart);
    }
}
