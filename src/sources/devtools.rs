//! Developer tools with their own update channels: editor extensions for VS
//! Code and its forks, gh extensions, and Google Cloud CLI components.

use std::fs;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use super::{Ctx, par_map};
use crate::copy;
use crate::dates;
use crate::exec;
use crate::http;
use crate::model::{Action, Discovery, Item, Release, Source};
use crate::origin::{self, Origin};
use crate::version;

/// Editor commands that manage VS Code style extensions.
const EDITORS: &[&str] = &[
    "code",
    "code-insiders",
    "cursor",
    "windsurf",
    "antigravity-ide",
    "codium",
    "kiro",
    "trae",
    "positron",
];

pub fn discover(_ctx: &Ctx) -> Discovery {
    let mut found = Discovery::default();
    let editors: Vec<&str> = EDITORS.iter().copied().filter(|e| exec::has(e)).collect();
    for items in par_map(editors, 4, editor_extensions) {
        found.items.extend(items);
    }
    found.items.extend(gh_extensions());
    found.items.extend(gcloud_components());
    found
}

fn cmd(program: &str, args: &[&str]) -> Action {
    Action::Command {
        program: program.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        sudo: false,
    }
}

// ---------- editor extensions ----------

#[derive(Deserialize)]
struct Product {
    version: Option<String>,
    #[serde(rename = "nameLong")]
    name_long: Option<String>,
    #[serde(rename = "extensionsGallery")]
    gallery: Option<Gallery>,
}

#[derive(Deserialize)]
struct Gallery {
    #[serde(rename = "serviceUrl")]
    service_url: String,
}

#[derive(Deserialize)]
struct QueryReply {
    #[serde(default)]
    results: Vec<QueryResult>,
}

#[derive(Deserialize)]
struct QueryResult {
    #[serde(default)]
    extensions: Vec<GalleryExtension>,
}

#[derive(Deserialize)]
struct GalleryExtension {
    #[serde(rename = "extensionName")]
    name: String,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    #[serde(rename = "shortDescription")]
    description: Option<String>,
    publisher: Publisher,
    #[serde(default)]
    versions: Vec<GalleryVersion>,
}

#[derive(Deserialize)]
struct Publisher {
    #[serde(rename = "publisherName")]
    name: String,
}

#[derive(Deserialize)]
struct GalleryVersion {
    version: String,
    #[serde(rename = "lastUpdated")]
    last_updated: Option<String>,
    #[serde(rename = "targetPlatform")]
    target_platform: Option<String>,
    #[serde(default)]
    properties: Vec<Property>,
}

#[derive(Deserialize)]
struct Property {
    key: String,
    value: String,
}

impl GalleryVersion {
    fn property(&self, key: &str) -> Option<&str> {
        self.properties
            .iter()
            .find(|p| p.key == key)
            .map(|p| p.value.as_str())
    }

    fn prerelease(&self) -> bool {
        self.property("Microsoft.VisualStudio.Code.PreRelease") == Some("true")
    }

    fn fits_platform(&self) -> bool {
        matches!(
            self.target_platform.as_deref(),
            None | Some("universal" | "darwin-arm64" | "web")
        )
    }

    /// Checks the `engines.vscode` range, such as `^1.95.0`, against the editor.
    fn fits_engine(&self, editor_version: &str) -> bool {
        match self.property("Microsoft.VisualStudio.Code.Engine") {
            Some(range) => {
                let min = range.trim_start_matches(['^', '>', '=', '~', ' ']);
                min == "*" || !version::is_newer(min, editor_version)
            }
            None => true,
        }
    }
}

fn editor_extensions(editor: &str) -> Vec<Item> {
    let Some(bin) = exec::which(editor) else {
        return Vec::new();
    };
    let Some(product) = product_json(&bin) else {
        return Vec::new();
    };
    let Some(gallery) = product.gallery else {
        return Vec::new();
    };
    let editor_version = product.version.unwrap_or_default();
    let editor_name = product.name_long.unwrap_or_else(|| editor.to_string());
    let out = exec::run(
        editor,
        &["--list-extensions", "--show-versions"],
        Duration::from_secs(60),
    );
    let installed: Vec<(String, String)> = out
        .stdout
        .lines()
        .filter_map(|l| l.trim().rsplit_once('@'))
        .map(|(id, v)| (id.to_lowercase(), v.to_string()))
        .collect();
    if installed.is_empty() {
        return Vec::new();
    }
    let criteria: Vec<serde_json::Value> = installed
        .iter()
        .map(|(id, _)| json!({"filterType": 7, "value": id}))
        .chain(std::iter::once(
            json!({"filterType": 8, "value": "Microsoft.VisualStudio.Code"}),
        ))
        .collect();
    let body = json!({
        "filters": [{
            "criteria": criteria,
            "pageNumber": 1,
            "pageSize": installed.len(),
            "sortBy": 0,
            "sortOrder": 0
        }],
        "assetTypes": [],
        // Flags 0x1 and 0x10 include versions and their properties.
        "flags": 0x11
    });
    let url = format!(
        "{}/extensionquery",
        gallery.service_url.trim_end_matches('/')
    );
    let reply: QueryReply = match http::post_json(
        &url,
        &[("Accept", "application/json;api-version=3.0-preview.1")],
        &body,
    ) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let mut items = Vec::new();
    for ext in reply.results.into_iter().flat_map(|r| r.extensions) {
        let id = format!("{}.{}", ext.publisher.name, ext.name).to_lowercase();
        let Some((_, current)) = installed.iter().find(|(i, _)| *i == id) else {
            continue;
        };
        let current_is_pre = ext
            .versions
            .iter()
            .find(|v| &v.version == current)
            .is_some_and(GalleryVersion::prerelease);
        // Users on a prerelease channel stay on it; everyone else gets stable only.
        let Some(latest) = ext.versions.iter().find(|v| {
            v.fits_platform()
                && v.fits_engine(&editor_version)
                && (current_is_pre || !v.prerelease())
        }) else {
            continue;
        };
        if !version::is_newer(&latest.version, current) {
            continue;
        }
        let date_of = |ver: &str| {
            ext.versions
                .iter()
                .find(|v| v.version == ver)
                .and_then(|v| v.last_updated.as_deref())
                .and_then(dates::parse)
        };
        items.push(
            Item::new(
                Source::DevTools,
                format!("ext:{editor}:{id}"),
                format!(
                    "{} ({editor_name})",
                    ext.display_name.clone().unwrap_or_else(|| id.clone())
                ),
                cmd(editor, &["--install-extension", &id, "--force"]),
            )
            .path(bin.display().to_string())
            .description(ext.description.clone().unwrap_or_default())
            .versions(
                Release::new(current, date_of(current)),
                Release::new(&latest.version, date_of(&latest.version)),
            ),
        );
    }
    items
}

/// Reads the editor's product.json, which names its extension gallery.
fn product_json(bin: &Path) -> Option<Product> {
    let app = match origin::of(bin) {
        Origin::App(app) => app,
        _ => {
            // Wrapper scripts in ~/.something/bin point at the app on their last line.
            let text = fs::read_to_string(bin).ok()?;
            let start = text.find("/Applications/")?;
            let end = text[start..].find(".app/")? + start + 4;
            text[start..end].into()
        }
    };
    let text = fs::read_to_string(app.join("Contents/Resources/app/product.json")).ok()?;
    serde_json::from_str(&text).ok()
}

// ---------- gh extensions ----------

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    published_at: Option<String>,
}

fn gh_extensions() -> Vec<Item> {
    if !exec::has("gh") {
        return Vec::new();
    }
    let out = exec::run("gh", &["extension", "list"], Duration::from_secs(20));
    // Lines hold the name, repository, and version, separated by tabs.
    let exts: Vec<(String, String, String)> = out
        .stdout
        .lines()
        .filter_map(|l| {
            let mut cols = l.split('\t');
            let name = cols.next()?.trim().trim_start_matches("gh ").to_string();
            let repo = cols.next()?.trim().to_string();
            let ver = cols.next()?.trim().to_string();
            Some((name, repo, ver))
        })
        // Extensions installed from a commit have no release to compare.
        .filter(|(_, _, ver)| {
            ver.trim_start_matches('v')
                .starts_with(|c: char| c.is_ascii_digit())
        })
        .collect();
    par_map(exts, 4, |(name, repo, current)| {
        let rel: GhRelease = http::github(&format!("repos/{repo}/releases/latest")).ok()?;
        if !version::is_newer(&rel.tag_name, &current) {
            return None;
        }
        let (owner, repo_name) = repo.split_once('/')?;
        Some(
            Item::new(
                Source::DevTools,
                format!("gh:{name}"),
                format!("gh {name}"),
                cmd("gh", &["extension", "upgrade", &name]),
            )
            .path(format!("https://github.com/{repo}"))
            .description(copy::DESC_GH_EXTENSION)
            .versions(
                Release::new(&current, dates::github_tag_date(owner, repo_name, &current)),
                Release::new(
                    &rel.tag_name,
                    rel.published_at.as_deref().and_then(dates::parse),
                ),
            ),
        )
    })
    .into_iter()
    .flatten()
    .collect()
}

// ---------- Google Cloud CLI ----------

#[derive(Deserialize)]
struct GcloudComponent {
    id: String,
    name: String,
    current_version_string: Option<String>,
    latest_version_string: Option<String>,
}

/// Components of a standalone Google Cloud CLI. When Homebrew installed it,
/// Homebrew's cask upgrade updates it instead.
fn gcloud_components() -> Vec<Item> {
    let Some(bin) = exec::which("gcloud") else {
        return Vec::new();
    };
    if matches!(
        origin::of(&bin),
        Origin::BrewCask(_) | Origin::BrewFormula(_)
    ) {
        return Vec::new();
    }
    let out = exec::run(
        "gcloud",
        &[
            "components",
            "list",
            "--format=json",
            "--filter=state.name='Update Available'",
            "--quiet",
        ],
        Duration::from_secs(120),
    );
    let list: Vec<GcloudComponent> = serde_json::from_str(&out.stdout).unwrap_or_default();
    list.into_iter()
        .map(|c| {
            Item::new(
                Source::DevTools,
                format!("gcloud:{}", c.id),
                &c.name,
                cmd("gcloud", &["components", "update", "--quiet"]),
            )
            .path(bin.display().to_string())
            .description(copy::DESC_GCLOUD)
            .versions(
                Release::new(c.current_version_string.unwrap_or_default(), None),
                Release::new(c.latest_version_string.unwrap_or_default(), None),
            )
        })
        .collect()
}
