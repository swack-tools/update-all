//! Homebrew's public cask catalog, used as a version source for apps that
//! Homebrew doesn't manage. update-all never adopts those apps into Homebrew.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use serde::Deserialize;

use crate::http;

const CASK_API: &str = "https://formulae.brew.sh/api/cask.json";

#[derive(Debug, Deserialize)]
pub struct Cask {
    pub token: String,
    pub desc: Option<String>,
    pub url: Option<String>,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub artifacts: Vec<serde_json::Value>,
    #[serde(default)]
    pub deprecated: bool,
    #[serde(default)]
    pub disabled: bool,
}

impl Cask {
    /// App bundle names this cask installs, such as `Google Chrome.app`.
    pub fn apps(&self) -> Vec<String> {
        app_artifacts(&self.artifacts)
    }

    /// The version without Homebrew's build suffix, or None when the cask has
    /// no fixed version.
    pub fn clean_version(&self) -> Option<&str> {
        let v = crate::version::clean(&self.version);
        (!v.is_empty() && self.version != "latest").then_some(v)
    }
}

/// Reads app names from a cask's `artifacts` list. An app entry has an `app`
/// list with the bundle name, and the installed name sits in a `target` key
/// either next to that list or inside it.
pub fn app_artifacts(artifacts: &[serde_json::Value]) -> Vec<String> {
    let mut out = Vec::new();
    for artifact in artifacts {
        let Some(list) = artifact.get("app").and_then(|v| v.as_array()) else {
            continue;
        };
        let target = list
            .iter()
            .find_map(|v| v.get("target").and_then(|t| t.as_str()))
            .or_else(|| artifact.get("target").and_then(|t| t.as_str()));
        if let Some(t) = target {
            out.push(t.rsplit('/').next().unwrap_or(t).to_string());
        } else if let Some(first) = list.first().and_then(|v| v.as_str()) {
            out.push(first.rsplit('/').next().unwrap_or(first).to_string());
        }
    }
    out
}

/// Bundle IDs that a cask's uninstall steps quit. Casks that install from a
/// pkg name their app only this way.
pub fn quit_ids(artifacts: &[serde_json::Value]) -> Vec<String> {
    let mut out = Vec::new();
    for steps in artifacts
        .iter()
        .filter_map(|a| a.get("uninstall")?.as_array())
    {
        for quit in steps.iter().filter_map(|s| s.get("quit")) {
            match quit {
                serde_json::Value::String(id) => out.push(id.clone()),
                serde_json::Value::Array(ids) => {
                    out.extend(ids.iter().filter_map(|i| i.as_str().map(str::to_string)))
                }
                _ => {}
            }
        }
    }
    out
}

/// Homebrew writes `You must reboot for the installation of X to take effect.`
pub fn caveats_need_reboot(caveats: &str) -> bool {
    let c = caveats.to_lowercase();
    c.contains("must reboot") || c.contains("restart your computer") || c.contains("reboot your")
}

pub struct Catalog {
    casks: Vec<Cask>,
    by_app: HashMap<String, Vec<usize>>,
}

impl Catalog {
    fn build(casks: Vec<Cask>) -> Catalog {
        let mut by_app: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, cask) in casks.iter().enumerate() {
            for app in cask.apps() {
                by_app.entry(app.to_lowercase()).or_default().push(i);
            }
        }
        Catalog { casks, by_app }
    }

    /// Finds the cask for an installed app. When several casks install an
    /// app with the same name, the one that mentions the bundle ID wins.
    pub fn for_app(&self, app_file: &str, bundle_id: &str) -> Option<&Cask> {
        let ids = self.by_app.get(&app_file.to_lowercase())?;
        let mut candidates: Vec<&Cask> = ids
            .iter()
            .map(|&i| &self.casks[i])
            .filter(|c| !c.disabled && c.clean_version().is_some())
            .collect();
        if candidates.len() > 1 && !bundle_id.is_empty() {
            let mentions: Vec<&Cask> = candidates
                .iter()
                .copied()
                .filter(|c| {
                    c.artifacts
                        .iter()
                        .any(|a| a.to_string().contains(&format!("\"{bundle_id}\"")))
                })
                .collect();
            if !mentions.is_empty() {
                candidates = mentions;
            }
        }
        candidates.sort_by_key(|c| (c.deprecated, c.token.contains('@'), c.token.len()));
        candidates.first().copied()
    }
}

/// The catalog, fetched once per run and cached on disk for three hours.
/// Returns None when offline with no cached copy.
pub fn get() -> Option<&'static Catalog> {
    static CATALOG: OnceLock<Option<Catalog>> = OnceLock::new();
    CATALOG
        .get_or_init(|| {
            let text = http::get_cached(CASK_API, Duration::from_secs(3 * 3600)).ok()?;
            let casks: Vec<Cask> = serde_json::from_str(&text).ok()?;
            Some(Catalog::build(casks))
        })
        .as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_app_artifacts() {
        let artifacts: Vec<serde_json::Value> = serde_json::from_str(
            r#"[{"app":["Google Chrome.app"]},{"zap":[{"trash":"~/x"}]},
                {"app":["dist/Foo.app",{"target":"Bar.app"}]},{"binary":["bin/codex"]},
                {"app":["Baz.app"],"target":"/Applications/Baz.app"}]"#,
        )
        .unwrap();
        assert_eq!(
            app_artifacts(&artifacts),
            vec!["Google Chrome.app", "Bar.app", "Baz.app"]
        );
    }

    #[test]
    fn reads_quit_ids() {
        let artifacts: Vec<serde_json::Value> = serde_json::from_str(
            r#"[{"uninstall":[{"quit":"io.tailscale.ipn.macsys","pkgutil":"x"}]},{"pkg":["T.pkg"]},
                {"uninstall":[{"quit":["a.b","c.d"]}]}]"#,
        )
        .unwrap();
        assert_eq!(
            quit_ids(&artifacts),
            vec!["io.tailscale.ipn.macsys", "a.b", "c.d"]
        );
    }

    #[test]
    fn detects_reboot_caveats() {
        assert!(caveats_need_reboot(
            "You must reboot for the installation of tailscale to take effect."
        ));
        assert!(!caveats_need_reboot("Run `foo init` to finish setup."));
    }

    #[test]
    fn picks_cask_by_bundle_id() {
        let casks: Vec<Cask> = serde_json::from_str(
            r#"[{"token":"foo-legacy","version":"1.0","artifacts":[{"app":["Foo.app"]}]},
                {"token":"foo","version":"2.0","artifacts":[{"app":["Foo.app"]},{"zap":[{"quit":"com.foo.app"}]}]}]"#,
        )
        .unwrap();
        let catalog = Catalog::build(casks);
        assert_eq!(
            catalog.for_app("Foo.app", "com.foo.app").unwrap().token,
            "foo"
        );
        assert_eq!(catalog.for_app("foo.app", "").unwrap().token, "foo");
        assert!(catalog.for_app("Bar.app", "").is_none());
    }
}
