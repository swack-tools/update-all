//! Apps in /Applications and ~/Applications that Homebrew doesn't manage:
//! browsers, AI desktop apps, and developer apps by default, and every app
//! when the user asks for every app. Each updates through its vendor's channel.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use super::sparkle::{self, Update};
use super::{Ctx, catalog, par_map};
use crate::copy::{self, fill};
use crate::dates;
use crate::exec;
use crate::http;
use crate::model::{Action, Discovery, Item, Release, Report, Source};
use crate::version;

/// What update-all reads from an app's Info.plist and bundle.
#[derive(Clone, Debug, Default)]
pub struct AppInfo {
    pub path: PathBuf,
    /// The filename without `.app`.
    pub name: String,
    pub bundle_id: String,
    /// CFBundleShortVersionString, the version people see.
    pub version: String,
    /// CFBundleVersion, which Sparkle compares.
    pub build: String,
    pub feed_url: Option<String>,
    pub ed_key: Option<String>,
    pub electron: bool,
    /// The app updates itself with Squirrel when it starts.
    pub squirrel: bool,
    pub mas: bool,
}

impl AppInfo {
    pub fn read(path: &Path) -> Option<AppInfo> {
        let plist = plist::Value::from_file(path.join("Contents/Info.plist")).ok()?;
        let dict = plist.as_dictionary()?;
        let get = |k: &str| dict.get(k).and_then(|v| v.as_string()).map(str::to_string);
        let build = get("CFBundleVersion").unwrap_or_default();
        let contents = path.join("Contents");
        Some(AppInfo {
            path: path.to_path_buf(),
            name: path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            bundle_id: get("CFBundleIdentifier").unwrap_or_default(),
            version: get("CFBundleShortVersionString").unwrap_or_else(|| build.clone()),
            build,
            feed_url: get("SUFeedURL"),
            ed_key: get("SUPublicEDKey"),
            electron: contents
                .join("Frameworks/Electron Framework.framework")
                .exists()
                || dict.contains_key("ElectronAsarIntegrity")
                || get("CFBundleIdentifier").is_some_and(|id| id.starts_with("com.electron.")),
            squirrel: contents.join("Frameworks/Squirrel.framework").exists(),
            mas: contents.join("_MASReceipt/receipt").exists(),
        })
    }

    fn file_name(&self) -> String {
        format!("{}.app", self.name)
    }
}

/// An app update-all checks by default: a browser, an AI desktop app, or a
/// developer app.
struct Known {
    /// Bundle ID, or `name:Foo.app` to match by filename.
    id: &'static str,
    description: &'static str,
    /// An appcast to use when the app doesn't name one in Info.plist.
    feed: Option<&'static str>,
}

const fn known(id: &'static str, description: &'static str) -> Known {
    Known {
        id,
        description,
        feed: None,
    }
}

const fn known_feed(id: &'static str, description: &'static str, feed: &'static str) -> Known {
    Known {
        id,
        description,
        feed: Some(feed),
    }
}

const KNOWN: &[Known] = &[
    // Browsers
    known("com.google.Chrome", "Web browser from Google"),
    known("org.mozilla.firefox", "Web browser from Mozilla"),
    known_feed(
        "com.brave.Browser",
        "Web browser that blocks ads and trackers",
        "https://updates.bravesoftware.com/sparkle/Brave-Browser/stable-arm64/appcast.xml",
    ),
    known("com.microsoft.edgemac", "Web browser from Microsoft"),
    known_feed(
        "company.thebrowser.Browser",
        "Arc web browser",
        "https://releases.arc.net/updates.xml",
    ),
    known("company.thebrowser.dia", "Dia web browser"),
    known_feed(
        "com.vivaldi.Vivaldi",
        "Vivaldi web browser",
        "https://update.vivaldi.com/update/1.0/public/mac/appcast.xml",
    ),
    known("com.operasoftware.Opera", "Opera web browser"),
    known(
        "org.chromium.Chromium",
        "Open source web browser behind Chrome",
    ),
    known("app.zen-browser.zen", "Zen web browser"),
    known("com.kagi.kagimacOS", "Orion web browser"),
    known("com.duckduckgo.macos.browser", "DuckDuckGo web browser"),
    known("org.torproject.torbrowser", "Tor Browser"),
    // AI desktop apps
    known(
        "com.anthropic.claudefordesktop",
        "Claude desktop app from Anthropic",
    ),
    known_feed(
        "com.openai.codex",
        "ChatGPT and Codex desktop app from OpenAI",
        "https://persistent.oaistatic.com/codex-app-prod/appcast.xml",
    ),
    known("com.openai.chat", "ChatGPT desktop app from OpenAI"),
    known("com.google.antigravity", "Google Antigravity desktop app"),
    known(
        "com.google.antigravity-ide",
        "Google Antigravity IDE, a code editor",
    ),
    known("com.todesktop.230313mzl4w4u92", "Cursor code editor"),
    known("com.exafunction.windsurf", "Windsurf code editor"),
    known("name:Devin.app", "Devin desktop app from Cognition"),
    known(
        "ai.elementlabs.bionic",
        "Bionic desktop app from Element Labs",
    ),
    known(
        "ai.elementlabs.lmstudio",
        "LM Studio, for running language models locally",
    ),
    known(
        "com.electron.ollama",
        "Ollama, for running language models locally",
    ),
    known("ai.perplexity.mac", "Perplexity desktop app"),
    known("dev.kiro.desktop", "Kiro code editor"),
    known("name:Trae.app", "Trae code editor"),
    // Developer apps
    known("com.microsoft.VSCode", "Visual Studio Code editor"),
    known("dev.zed.Zed", "Zed code editor"),
    known("dev.warp.Warp-Stable", "Warp terminal"),
    known("com.mitchellh.ghostty", "Ghostty terminal"),
    known("com.googlecode.iterm2", "iTerm2 terminal"),
    known("com.github.GitHubClient", "GitHub Desktop"),
    known("com.docker.docker", "Docker Desktop"),
    known(
        "dev.kdrag0n.MacVirt",
        "OrbStack, which runs containers and Linux machines",
    ),
    known("com.jetbrains.toolbox", "JetBrains Toolbox"),
    known("com.postmanlabs.mac", "Postman API client"),
    known("com.sublimetext.4", "Sublime Text editor"),
    known("com.tinyapp.TablePlus", "TablePlus database client"),
];

/// Registry descriptions, for the Vale check.
#[cfg(test)]
pub fn described() -> Vec<(String, &'static str)> {
    KNOWN
        .iter()
        .map(|k| (format!("app {}", k.id), k.description))
        .collect()
}

fn find_known(app: &AppInfo) -> Option<&'static Known> {
    KNOWN.iter().find(|k| match k.id.strip_prefix("name:") {
        Some(file) => file.eq_ignore_ascii_case(&app.file_name()),
        None => k.id == app.bundle_id,
    })
}

/// Every app bundle in the usual folders, one level of subfolders deep.
pub fn installed_apps(ctx: &Ctx) -> Vec<AppInfo> {
    let mut out = Vec::new();
    for dir in [
        PathBuf::from("/Applications"),
        exec::home().join("Applications"),
    ] {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if ctx.config.ignored(&p) {
                continue;
            }
            if p.extension().is_some_and(|e| e == "app") {
                out.extend(AppInfo::read(&p));
            } else if p.is_dir() && p.file_name().is_some_and(|n| n != "Utilities") {
                for sub in fs::read_dir(&p).into_iter().flatten().flatten() {
                    let sp = sub.path();
                    if sp.extension().is_some_and(|e| e == "app") && !ctx.config.ignored(&sp) {
                        out.extend(AppInfo::read(&sp));
                    }
                }
            }
        }
    }
    out
}

#[derive(Deserialize, Default)]
struct BrewInstalled {
    #[serde(default)]
    casks: Vec<BrewCask>,
}

#[derive(Deserialize)]
struct BrewCask {
    #[serde(default)]
    artifacts: Vec<serde_json::Value>,
}

/// App file names that Homebrew casks installed. Homebrew updates those.
fn brew_owned() -> HashSet<String> {
    if !exec::has("brew") {
        return HashSet::new();
    }
    let out = exec::run(
        "brew",
        &["info", "--json=v2", "--installed", "--cask"],
        Duration::from_secs(120),
    );
    serde_json::from_str::<BrewInstalled>(&out.stdout)
        .unwrap_or_default()
        .casks
        .iter()
        .flat_map(|c| catalog::app_artifacts(&c.artifacts))
        .map(|n| n.to_lowercase())
        .collect()
}

fn os_version() -> String {
    exec::run("sw_vers", &["-productVersion"], Duration::from_secs(5))
        .stdout
        .trim()
        .to_string()
}

/// What update-all learned about an app's newest version.
struct Latest {
    current: Release,
    latest: Release,
    sparkle: Option<sparkle::FeedItem>,
}

pub fn discover(ctx: &Ctx) -> Discovery {
    let mut found = Discovery::default();
    let owned = brew_owned();
    let apps: Vec<AppInfo> = installed_apps(ctx)
        .into_iter()
        .filter(|a| !a.mas && !a.bundle_id.starts_with("com.apple."))
        .filter(|a| !owned.contains(&a.file_name().to_lowercase()))
        .filter(|a| ctx.literally_all || find_known(a).is_some())
        .collect();
    if apps.is_empty() {
        return found;
    }
    let os = os_version();
    let results = par_map(apps, 8, |app| {
        let latest = latest_for(&app, &os)?;
        Some((app, latest))
    });
    let mut manual = Vec::new();
    for (app, latest) in results.into_iter().flatten() {
        let cask = catalog::get().and_then(|c| c.for_app(&app.file_name(), &app.bundle_id));
        let description = cask
            .and_then(|c| c.desc.clone())
            .or_else(|| find_known(&app).map(|k| k.description.to_string()))
            .unwrap_or_default();
        let Some(action) = action_for(&app, &latest) else {
            manual.push(fill(
                copy::REPORT_MANUAL_LINE,
                &[&app.name, &latest.current.version, &latest.latest.version],
            ));
            continue;
        };
        let mut item = Item::new(Source::Apps, format!("app:{}", app.name), &app.name, action)
            .path(app.path.display().to_string())
            .description(description)
            .versions(latest.current, latest.latest);
        item.electron = app.electron;
        if matches!(item.action, Action::LaunchHidden { .. }) {
            item.note = Some(copy::NOTE_LAUNCH_HIDDEN.to_string());
        }
        found.items.push(item);
    }
    if !manual.is_empty() {
        manual.insert(0, copy::REPORT_MANUAL_WHY.to_string());
        found.reports.push(Report {
            title: copy::REPORT_MANUAL_TITLE.to_string(),
            lines: manual,
        });
    }
    found
}

/// Finds the newest version from the most reliable source for this app. Returns None
/// when the app is current or nothing knows about it.
fn latest_for(app: &AppInfo, os: &str) -> Option<Latest> {
    match app.bundle_id.as_str() {
        "com.google.Chrome" => return chrome(app),
        "org.mozilla.firefox" => return firefox(app),
        _ => {}
    }
    let feed = app
        .feed_url
        .clone()
        .or_else(|| find_known(app).and_then(|k| k.feed.map(str::to_string)));
    if let Some(url) = feed {
        let items = sparkle::fetch_feed(&url, os);
        if let Some(best) = items.first() {
            // Sparkle compares bundle versions, so update-all does too.
            if !version::is_newer(&best.build, &app.build) {
                return None;
            }
            let cur_date = items
                .iter()
                .find(|i| i.build == app.build || i.display == app.version)
                .and_then(|i| i.date);
            return Some(Latest {
                current: Release::new(&app.version, cur_date),
                latest: Release::new(&best.display, best.date),
                sparkle: Some(best.clone()),
            });
        }
    }
    let cask = catalog::get()?.for_app(&app.file_name(), &app.bundle_id)?;
    let newest = cask.clean_version()?;
    if !version::is_newer(newest, &app.version) {
        return None;
    }
    let (cur_date, new_date) = cask
        .url
        .as_deref()
        .and_then(|url| {
            let (owner, repo) = dates::github_repo(url)?;
            let tag = dates::github_tag(url)?;
            Some(dates::github_pair(
                &owner,
                &repo,
                &tag,
                newest,
                version::clean(&app.version),
            ))
        })
        .unwrap_or((None, None));
    Some(Latest {
        current: Release::new(&app.version, cur_date),
        latest: Release::new(newest, new_date),
        sparkle: None,
    })
}

fn action_for(app: &AppInfo, latest: &Latest) -> Option<Action> {
    let path = app.path.display().to_string();
    if app.bundle_id == "com.google.Chrome" {
        return google_updater();
    }
    if let Some(id) = mau_id(&app.bundle_id)
        && let Some(msupdate) = msupdate()
    {
        return Some(Action::Command {
            program: msupdate,
            args: vec![
                "--install".to_string(),
                "--apps".to_string(),
                id.to_string(),
            ],
            sudo: false,
        });
    }
    if let (Some(item), Some(key)) = (&latest.sparkle, &app.ed_key)
        && item.ed_signature.is_some()
    {
        return Some(Action::Sparkle(Update {
            app: app.path.clone(),
            bundle_id: app.bundle_id.clone(),
            item: item.clone(),
            public_key: key.clone(),
        }));
    }
    if app.electron || app.squirrel {
        // Electron and Squirrel apps check for updates when they start.
        return Some(Action::LaunchHidden { app: path });
    }
    None
}

// ---------- Chrome ----------

#[derive(Deserialize)]
struct ChromeReleases {
    #[serde(default)]
    releases: Vec<ChromeRelease>,
}

#[derive(Deserialize)]
struct ChromeRelease {
    version: String,
    serving: Option<Serving>,
}

#[derive(Deserialize)]
struct Serving {
    #[serde(rename = "startTime")]
    start_time: Option<String>,
}

const CHROME_API: &str = "https://versionhistory.googleapis.com/v1/chrome/platforms/mac_arm64/channels/stable/versions/all/releases";

/// Chrome's newest fully rolled out stable version, from Google's Version History API.
fn chrome(app: &AppInfo) -> Option<Latest> {
    let full: ChromeReleases =
        http::get_json(&format!("{CHROME_API}?filter=endtime=none,fraction=1")).ok()?;
    let newest = full
        .releases
        .iter()
        .map(|r| r.version.as_str())
        .max_by(|a, b| version::compare(a, b))?
        .to_string();
    if !version::is_newer(&newest, &app.version) {
        return None;
    }
    // A version's first serving record marks its release.
    let first_served = |v: &str| -> Option<jiff::Timestamp> {
        let r: ChromeReleases = http::get_json(&format!(
            "{CHROME_API}?filter=version={v}&order_by=starttime%20asc&pageSize=1"
        ))
        .ok()?;
        r.releases
            .first()?
            .serving
            .as_ref()?
            .start_time
            .as_deref()
            .and_then(dates::parse)
    };
    Some(Latest {
        current: Release::new(&app.version, first_served(&app.version)),
        latest: Release::new(&newest, first_served(&newest)),
        sparkle: None,
    })
}

/// GoogleUpdater's `--update-apps` checks and installs right away.
fn google_updater() -> Option<Action> {
    let rel = "Google/GoogleUpdater/Current/GoogleUpdater.app/Contents/MacOS/GoogleUpdater";
    let user = exec::home().join("Library/Application Support").join(rel);
    if user.exists() {
        return Some(Action::Command {
            program: user.display().to_string(),
            args: vec!["--update-apps".to_string()],
            sudo: false,
        });
    }
    let system = Path::new("/Library/Application Support").join(rel);
    system.exists().then(|| Action::Command {
        program: system.display().to_string(),
        args: vec!["--update-apps".to_string(), "--system".to_string()],
        sudo: false,
    })
}

// ---------- Firefox ----------

#[derive(Deserialize)]
struct FirefoxVersions {
    #[serde(rename = "LATEST_FIREFOX_VERSION")]
    latest: String,
}

const MOZILLA: &str = "https://product-details.mozilla.org/1.0";

fn firefox(app: &AppInfo) -> Option<Latest> {
    let v: FirefoxVersions = http::get_json(&format!("{MOZILLA}/firefox_versions.json")).ok()?;
    if !version::is_newer(&v.latest, &app.version) {
        return None;
    }
    let mut history: BTreeMap<String, String> = BTreeMap::new();
    for file in [
        "firefox_history_major_releases.json",
        "firefox_history_stability_releases.json",
    ] {
        if let Ok(map) = http::get_json::<BTreeMap<String, String>>(&format!("{MOZILLA}/{file}")) {
            history.extend(map);
        }
    }
    let date = |v: &str| history.get(v).and_then(|d| dates::parse(d));
    Some(Latest {
        current: Release::new(&app.version, date(&app.version)),
        latest: Release::new(&v.latest, date(&v.latest)),
        sparkle: None,
    })
}

// ---------- Microsoft's update tool, msupdate ----------

fn msupdate() -> Option<String> {
    let p = "/Library/Application Support/Microsoft/MAU2.0/Microsoft AutoUpdate.app/Contents/MacOS/msupdate";
    Path::new(p).exists().then(|| p.to_string())
}

/// The app IDs that msupdate uses.
fn mau_id(bundle_id: &str) -> Option<&'static str> {
    Some(match bundle_id {
        "com.microsoft.edgemac" => "EDGE01",
        "com.microsoft.Word" => "MSWD2019",
        "com.microsoft.Excel" => "XCEL2019",
        "com.microsoft.Powerpoint" => "PPT32019",
        "com.microsoft.Outlook" => "OPIM2019",
        "com.microsoft.onenote.mac" => "ONMC2019",
        "com.microsoft.OneDrive" => "ONDR18",
        "com.microsoft.teams2" => "TEAMS21",
        "com.microsoft.rdc.macos" => "MSRD10",
        _ => return None,
    })
}
