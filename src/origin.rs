//! Works out which installer owns a binary, so only that installer updates it.

use std::fs;
use std::path::{Path, PathBuf};

use crate::exec;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// A Homebrew formula, by name.
    BrewFormula(String),
    /// A Homebrew cask, by token.
    BrewCask(String),
    /// A global npm package under Homebrew's or another Node prefix.
    Npm(String),
    /// A uv tool, by tool name.
    Uv(String),
    /// A pipx app, by venv name.
    Pipx(String),
    /// A Bun global package.
    Bun,
    /// A `cargo install` binary.
    Cargo,
    /// Part of macOS or the Xcode tools.
    System,
    /// Inside an app bundle, by app path.
    App(PathBuf),
    /// Installed by its own installer, which also updates it.
    Native,
}

/// Follows symlinks one hop at a time and classifies each path. Homebrew
/// links can point through its Caskroom into another folder, so the first
/// hop that names an installer wins over the final path.
pub fn of(binary: &Path) -> Origin {
    let home = exec::home();
    let mut current = binary.to_path_buf();
    for _ in 0..8 {
        let origin = classify(&current, &home);
        if !matches!(origin, Origin::Native | Origin::System) {
            return origin;
        }
        let Ok(target) = fs::read_link(&current) else {
            break;
        };
        current = match current.parent() {
            Some(dir) if target.is_relative() => dir.join(target),
            _ => target,
        };
    }
    let real = fs::canonicalize(binary).unwrap_or_else(|_| binary.to_path_buf());
    classify(&real, &home)
}

pub fn classify(real: &Path, home: &Path) -> Origin {
    let s = real.to_string_lossy();
    let after = |marker: &str| -> Option<String> {
        s.split_once(marker)
            .and_then(|(_, rest)| rest.split('/').next())
            .map(str::to_string)
    };
    for prefix in ["/opt/homebrew", "/usr/local"] {
        if let Some(name) = after(&format!("{prefix}/Cellar/")) {
            return Origin::BrewFormula(name);
        }
        if let Some(token) = after(&format!("{prefix}/Caskroom/")) {
            return Origin::BrewCask(token);
        }
    }
    if let Some((_, rest)) = s.split_once("/lib/node_modules/") {
        return Origin::Npm(npm_name(rest));
    }
    if s.contains("/.bun/install/global/") {
        return Origin::Bun;
    }
    if let Some(name) = after(&format!("{}/.local/share/uv/tools/", home.display())) {
        return Origin::Uv(name);
    }
    if let Some(name) = after(&format!("{}/.local/pipx/venvs/", home.display()))
        .or_else(|| after(&format!("{}/.local/share/pipx/venvs/", home.display())))
    {
        return Origin::Pipx(name);
    }
    if real.starts_with(home.join(".cargo/bin")) {
        return Origin::Cargo;
    }
    if let Some(idx) = s.find(".app/") {
        return Origin::App(PathBuf::from(&s[..idx + 4]));
    }
    if [
        "/usr/bin/",
        "/bin/",
        "/usr/sbin/",
        "/sbin/",
        "/System/",
        "/Library/Developer/",
    ]
    .iter()
    .any(|p| s.starts_with(p))
    {
        return Origin::System;
    }
    Origin::Native
}

/// `@scope/name/bin/x.js` becomes `@scope/name`; `name/bin/x` becomes `name`.
fn npm_name(rest: &str) -> String {
    let mut parts = rest.split('/');
    let first = parts.next().unwrap_or_default();
    if first.starts_with('@') {
        format!("{first}/{}", parts.next().unwrap_or_default())
    } else {
        first.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(path: &str) -> Origin {
        classify(Path::new(path), Path::new("/Users/me"))
    }

    #[test]
    fn homebrew() {
        assert_eq!(
            class("/opt/homebrew/Cellar/wget/1.25.0/bin/wget"),
            Origin::BrewFormula("wget".into())
        );
        assert_eq!(
            class("/opt/homebrew/Caskroom/ngrok/3.18.4/ngrok"),
            Origin::BrewCask("ngrok".into())
        );
    }

    #[test]
    fn package_managers() {
        assert_eq!(
            class("/opt/homebrew/lib/node_modules/@angular/cli/bin/ng.js"),
            Origin::Npm("@angular/cli".into())
        );
        assert_eq!(
            class("/opt/homebrew/lib/node_modules/typescript/bin/tsc"),
            Origin::Npm("typescript".into())
        );
        assert_eq!(
            class("/Users/me/.local/share/uv/tools/ruff/bin/ruff"),
            Origin::Uv("ruff".into())
        );
        assert_eq!(
            class("/Users/me/.local/pipx/venvs/black/bin/black"),
            Origin::Pipx("black".into())
        );
        assert_eq!(class("/Users/me/.cargo/bin/rg"), Origin::Cargo);
    }

    #[test]
    fn native_system_and_apps() {
        assert_eq!(
            class("/Users/me/.local/share/claude/versions/2.1.0"),
            Origin::Native
        );
        assert_eq!(class("/Users/me/.local/bin/opencode"), Origin::Native);
        assert_eq!(class("/usr/bin/swift"), Origin::System);
        assert_eq!(
            class("/Applications/Docker.app/Contents/Resources/bin/docker"),
            Origin::App(PathBuf::from("/Applications/Docker.app"))
        );
    }
}
