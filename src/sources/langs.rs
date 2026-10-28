//! Language package managers: npm, pipx, uv, cargo, rustup, Bun, gem, pip,
//! which update-all only reports, and the managers that update themselves.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use super::{Ctx, par_map};
use crate::copy::{self, fill};
use crate::dates;
use crate::exec;
use crate::http;
use crate::model::{Action, Discovery, Item, Release, Report, Source};
use crate::origin::{self, Origin};
use crate::version;

const QUICK: Duration = Duration::from_secs(30);
const SLOW: Duration = Duration::from_secs(120);

pub fn discover(_ctx: &Ctx) -> Discovery {
    let mut found = Discovery::default();
    let parts: Vec<fn() -> Discovery> =
        vec![npm, pipx, uv, pip, cargo, rustup, bun, gem, self_updaters];
    for part in par_map(parts, 9, |f| f()) {
        found.items.extend(part.items);
        found.reports.extend(part.reports);
        found.notes.extend(part.notes);
    }
    found
}

fn cmd(program: &str, args: &[&str]) -> Action {
    Action::Command {
        program: program.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        sudo: false,
    }
}

// ---------- registries ----------

/// What a package registry knows about a package.
pub struct Meta {
    pub latest: String,
    pub description: String,
    pub dates: BTreeMap<String, String>,
}

impl Meta {
    pub fn date(&self, version: &str) -> Option<jiff::Timestamp> {
        self.dates.get(version).and_then(|d| dates::parse(d))
    }

    pub fn releases(&self, current: &str) -> (Release, Release) {
        (
            Release::new(current, self.date(current)),
            Release::new(&self.latest, self.date(&self.latest)),
        )
    }
}

#[derive(Deserialize)]
struct NpmDoc {
    #[serde(rename = "dist-tags", default)]
    dist_tags: BTreeMap<String, String>,
    #[serde(default)]
    time: BTreeMap<String, String>,
    description: Option<String>,
}

/// Looks up a package on the npm registry.
pub fn npm_meta(package: &str) -> Option<Meta> {
    let url = format!("https://registry.npmjs.org/{}", package.replace('/', "%2F"));
    let doc: NpmDoc = http::get_json(&url).ok()?;
    Some(Meta {
        latest: doc.dist_tags.get("latest")?.clone(),
        description: doc.description.unwrap_or_default(),
        dates: doc.time,
    })
}

#[derive(Deserialize)]
struct PypiDoc {
    info: PypiInfo,
    #[serde(default)]
    releases: BTreeMap<String, Vec<PypiFile>>,
}

#[derive(Deserialize)]
struct PypiInfo {
    version: String,
    summary: Option<String>,
}

#[derive(Deserialize)]
struct PypiFile {
    upload_time_iso_8601: Option<String>,
}

/// Looks up a package on PyPI.
pub fn pypi_meta(package: &str) -> Option<Meta> {
    let doc: PypiDoc = http::get_json(&format!("https://pypi.org/pypi/{package}/json")).ok()?;
    let dates = doc
        .releases
        .into_iter()
        .filter_map(|(v, files)| {
            files
                .into_iter()
                .find_map(|f| f.upload_time_iso_8601)
                .map(|d| (v, d))
        })
        .collect();
    Some(Meta {
        latest: doc.info.version,
        description: doc.info.summary.unwrap_or_default(),
        dates,
    })
}

#[derive(Deserialize)]
struct CratesDoc {
    #[serde(rename = "crate")]
    krate: CrateInfo,
    #[serde(default)]
    versions: Vec<CrateVersion>,
}

#[derive(Deserialize)]
struct CrateInfo {
    max_stable_version: Option<String>,
    max_version: String,
    description: Option<String>,
}

#[derive(Deserialize)]
struct CrateVersion {
    num: String,
    created_at: String,
}

fn crates_meta(name: &str) -> Option<Meta> {
    let doc: CratesDoc = http::get_json(&format!("https://crates.io/api/v1/crates/{name}")).ok()?;
    Some(Meta {
        latest: doc
            .krate
            .max_stable_version
            .unwrap_or(doc.krate.max_version),
        description: doc.krate.description.unwrap_or_default(),
        dates: doc
            .versions
            .into_iter()
            .map(|v| (v.num, v.created_at))
            .collect(),
    })
}

/// The newest stable version from the crates.io sparse index. The index is a
/// set of static files, so it has no rate limit, unlike the crates.io API.
fn crates_index_latest(name: &str) -> Option<String> {
    let n = name.to_lowercase();
    let path = match n.len() {
        1 => format!("1/{n}"),
        2 => format!("2/{n}"),
        3 => format!("3/{}/{n}", &n[..1]),
        _ => format!("{}/{}/{n}", &n[..2], &n[2..4]),
    };
    let text = http::get_text(&format!("https://index.crates.io/{path}")).ok()?;
    #[derive(Deserialize)]
    struct Line {
        vers: String,
        #[serde(default)]
        yanked: bool,
    }
    text.lines()
        .filter_map(|l| serde_json::from_str::<Line>(l).ok())
        .filter(|l| !l.yanked && !l.vers.contains('-'))
        .map(|l| l.vers)
        .max_by(|a, b| version::compare(a, b))
}

// ---------- npm ----------

#[derive(Deserialize)]
struct NpmOutdated {
    current: Option<String>,
    latest: Option<String>,
    location: Option<String>,
}

fn npm() -> Discovery {
    let mut found = Discovery::default();
    let Some(npm_bin) = exec::which("npm") else {
        return found;
    };
    let node_from_brew = exec::which("node")
        .map(|n| matches!(origin::of(&n), Origin::BrewFormula(_)))
        .unwrap_or(false);
    // `npm outdated` exits with status 1 when it finds anything to update.
    let out = exec::run("npm", &["outdated", "-g", "--json"], SLOW);
    let list: BTreeMap<String, NpmOutdated> = serde_json::from_str(&out.stdout).unwrap_or_default();
    let jobs: Vec<(String, NpmOutdated)> = list
        .into_iter()
        // Homebrew's node ships npm and corepack, so Homebrew updates them.
        .filter(|(name, _)| !(node_from_brew && (name == "npm" || name == "corepack")))
        .collect();
    found.items = par_map(jobs, 6, |(name, entry)| {
        let current = entry.current.clone()?;
        let meta = npm_meta(&name);
        let latest = meta
            .as_ref()
            .map(|m| m.latest.clone())
            .or(entry.latest.clone())?;
        if !version::is_newer(&latest, &current) {
            return None;
        }
        let (cur, new) = match &meta {
            Some(m) => (
                Release::new(&current, m.date(&current)),
                Release::new(&latest, m.date(&latest)),
            ),
            None => (Release::new(&current, None), Release::new(&latest, None)),
        };
        Some(
            Item::new(
                Source::Langs,
                format!("npm:{name}"),
                &name,
                cmd(
                    &npm_bin.display().to_string(),
                    &["install", "-g", &format!("{name}@{latest}")],
                ),
            )
            .path(entry.location.unwrap_or_default())
            .description(meta.map(|m| m.description).unwrap_or_default())
            .versions(cur, new),
        )
    })
    .into_iter()
    .flatten()
    .collect();
    found
}

// ---------- pipx ----------

#[derive(Deserialize)]
struct PipxList {
    #[serde(default)]
    venvs: BTreeMap<String, PipxVenv>,
}

#[derive(Deserialize)]
struct PipxVenv {
    metadata: PipxMetadata,
}

#[derive(Deserialize)]
struct PipxMetadata {
    main_package: PipxPackage,
}

#[derive(Deserialize)]
struct PipxPackage {
    package: String,
    package_version: String,
    package_or_url: Option<String>,
}

/// True for specs that point at a folder, a file, or a Git URL instead of PyPI.
fn is_local_spec(spec: &str) -> bool {
    spec.contains('/')
        || spec.contains("git+")
        || spec.ends_with(".whl")
        || spec.ends_with(".tar.gz")
}

fn pipx() -> Discovery {
    let mut found = Discovery::default();
    if !exec::has("pipx") {
        return found;
    }
    let out = exec::run("pipx", &["list", "--json"], QUICK);
    let Ok(list) = serde_json::from_str::<PipxList>(&out.stdout) else {
        return found;
    };
    let jobs: Vec<(String, PipxPackage)> = list
        .venvs
        .into_iter()
        .map(|(venv, v)| (venv, v.metadata.main_package))
        .filter(|(_, p)| !p.package_or_url.as_deref().is_some_and(is_local_spec))
        .collect();
    found.items = par_map(jobs, 6, |(venv, pkg)| {
        let meta = pypi_meta(&pkg.package)?;
        if !version::is_newer(&meta.latest, &pkg.package_version) {
            return None;
        }
        let (cur, new) = meta.releases(&pkg.package_version);
        Some(
            Item::new(
                Source::Langs,
                format!("pipx:{venv}"),
                &venv,
                cmd("pipx", &["upgrade", &venv]),
            )
            .path(
                exec::home()
                    .join(".local/pipx/venvs")
                    .join(&venv)
                    .display()
                    .to_string(),
            )
            .description(meta.description)
            .versions(cur, new),
        )
    })
    .into_iter()
    .flatten()
    .collect();
    found
}

// ---------- uv ----------

fn uv() -> Discovery {
    let mut found = Discovery::default();
    if !exec::has("uv") {
        return found;
    }
    let dir = exec::run("uv", &["tool", "dir"], QUICK)
        .stdout
        .trim()
        .to_string();
    let out = exec::run("uv", &["tool", "list"], QUICK);
    let tools: Vec<(String, String)> = out
        .stdout
        .lines()
        .filter(|l| !l.starts_with('-') && !l.trim().is_empty())
        .filter_map(|l| {
            let mut words = l.split_whitespace();
            let name = words.next()?.to_string();
            let ver = words.next()?.trim_start_matches('v').to_string();
            Some((name, ver))
        })
        .filter(|(name, _)| !uv_tool_is_local(&dir, name))
        .collect();
    found.items = par_map(tools, 6, |(name, current)| {
        let meta = pypi_meta(&name)?;
        if !version::is_newer(&meta.latest, &current) {
            return None;
        }
        let (cur, new) = meta.releases(&current);
        Some(
            Item::new(
                Source::Langs,
                format!("uv:{name}"),
                &name,
                cmd("uv", &["tool", "upgrade", &name]),
            )
            .path(Path::new(&dir).join(&name).display().to_string())
            .description(meta.description)
            .versions(cur, new),
        )
    })
    .into_iter()
    .flatten()
    .collect();
    found
}

/// uv records where each tool came from. Tools from a folder, a Git URL, or
/// an editable install have nothing to compare on PyPI.
fn uv_tool_is_local(tool_dir: &str, name: &str) -> bool {
    let receipt = Path::new(tool_dir).join(name).join("uv-receipt.toml");
    fs::read_to_string(receipt).is_ok_and(|t| {
        ["path =", "git =", "url =", "editable ="]
            .iter()
            .any(|k| t.contains(k))
    })
}

// ---------- pip, report only ----------

#[derive(Deserialize)]
struct PipOutdated {
    name: String,
    version: String,
    latest_version: String,
}

fn pip() -> Discovery {
    let mut found = Discovery::default();
    let Some(pip) = ["pip3", "pip"].into_iter().find(|p| exec::has(p)) else {
        return found;
    };
    let out = exec::run(
        pip,
        &[
            "list",
            "--outdated",
            "--format=json",
            "--disable-pip-version-check",
        ],
        SLOW,
    );
    let list: Vec<PipOutdated> = serde_json::from_str(&out.stdout).unwrap_or_default();
    if list.is_empty() {
        return found;
    }
    let mut report = Report {
        title: fill(copy::REPORT_PIP_TITLE, &[pip]),
        lines: vec![copy::REPORT_PIP_WHY.to_string()],
    };
    for p in list {
        report
            .lines
            .push(format!("{}  {} -> {}", p.name, p.version, p.latest_version));
    }
    found.reports.push(report);
    found
}

// ---------- cargo ----------

fn cargo() -> Discovery {
    let mut found = Discovery::default();
    if !exec::has("cargo") {
        return found;
    }
    let out = exec::run("cargo", &["install", "--list"], QUICK);
    // Lines look like `just v1.58.0:`. Crates from a path or Git add their
    // source after the version.
    let crates: Vec<(String, String)> = out
        .stdout
        .lines()
        .filter(|l| !l.starts_with(' ') && !l.contains('('))
        .filter_map(|l| {
            let (name, ver) = l.trim_end_matches(':').split_once(' ')?;
            Some((name.to_string(), ver.trim_start_matches('v').to_string()))
        })
        .collect();
    let updater = exec::has("cargo-install-update");
    found.items = par_map(crates, 8, |(name, current)| {
        let latest = crates_index_latest(&name)?;
        if !version::is_newer(&latest, &current) {
            return None;
        }
        let meta = crates_meta(&name);
        let (cur, new) = match &meta {
            Some(m) => (
                Release::new(&current, m.date(&current)),
                Release::new(&latest, m.date(&latest)),
            ),
            None => (Release::new(&current, None), Release::new(&latest, None)),
        };
        let action = if updater {
            cmd("cargo", &["install-update", &name])
        } else {
            cmd("cargo", &["install", &name, "--locked"])
        };
        Some(
            Item::new(Source::Langs, format!("cargo:{name}"), &name, action)
                .path(exec::home().join(".cargo/bin").display().to_string())
                .description(meta.map(|m| m.description).unwrap_or_default())
                .versions(cur, new),
        )
    })
    .into_iter()
    .flatten()
    .collect();
    found
}

// ---------- rustup ----------

fn rustup() -> Discovery {
    let mut found = Discovery::default();
    let Some(rustup_bin) = exec::which("rustup") else {
        return found;
    };
    if let Some(rustc) = exec::which("rustc")
        && let Origin::BrewFormula(_) = origin::of(&rustc)
    {
        found
            .notes
            .push(copy::NOTE_RUSTUP_AND_BREW_RUST.to_string());
    }
    let out = exec::run("rustup", &["check"], SLOW);
    for line in out.stdout.lines() {
        // Lines look like `stable-aarch64-apple-darwin - Update available : 1.98.0 -> 1.99.0`,
        // where each version has a commit hash and a date after it.
        let Some((name, rest)) = line.split_once(" - Update available : ") else {
            continue;
        };
        let Some((from, to)) = rest.split_once(" -> ") else {
            continue;
        };
        let parse = |s: &str| {
            let ver = s.split_whitespace().next().unwrap_or_default().to_string();
            let date = s
                .split_whitespace()
                .last()
                .map(|d| d.trim_end_matches(')'))
                .and_then(dates::parse);
            Release::new(ver, date)
        };
        let name = name.trim();
        found.items.push(
            Item::new(
                Source::Langs,
                format!("rustup:{name}"),
                name,
                cmd("rustup", &["update", name]),
            )
            .path(rustup_bin.display().to_string())
            .description(copy::DESC_RUSTUP)
            .versions(parse(from), parse(to)),
        );
    }
    found
}

// ---------- Bun ----------

#[derive(Deserialize)]
struct BunGlobal {
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
}

fn bun() -> Discovery {
    let mut found = Discovery::default();
    if !exec::has("bun") {
        return found;
    }
    let dir = exec::home().join(".bun/install/global");
    let Ok(text) = fs::read_to_string(dir.join("package.json")) else {
        return found;
    };
    let Ok(global) = serde_json::from_str::<BunGlobal>(&text) else {
        return found;
    };
    let jobs: Vec<String> = global.dependencies.into_keys().collect();
    found.items = par_map(jobs, 6, |name| {
        let pkg_json = dir.join("node_modules").join(&name).join("package.json");
        let current: String = fs::read_to_string(pkg_json)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| v.get("version")?.as_str().map(str::to_string))?;
        let meta = npm_meta(&name)?;
        if !version::is_newer(&meta.latest, &current) {
            return None;
        }
        let (cur, new) = meta.releases(&current);
        let latest = meta.latest.clone();
        Some(
            Item::new(
                Source::Langs,
                format!("bun:{name}"),
                &name,
                cmd("bun", &["add", "-g", &format!("{name}@{latest}")]),
            )
            .path(dir.join("node_modules").join(&name).display().to_string())
            .description(meta.description)
            .versions(cur, new),
        )
    })
    .into_iter()
    .flatten()
    .collect();
    found
}

// ---------- gem ----------

#[derive(Deserialize)]
struct GemVersion {
    number: String,
    created_at: String,
    summary: Option<String>,
    #[serde(default)]
    prerelease: bool,
}

fn gem() -> Discovery {
    let mut found = Discovery::default();
    let Some(gem_bin) = exec::which("gem") else {
        return found;
    };
    // macOS's own Ruby belongs to the system. Leave it alone.
    if origin::of(&gem_bin) == Origin::System {
        return found;
    }
    // Gems that ship with Homebrew's Ruby sit in its Cellar folder, and the
    // ruby formula updates them.
    let bundled: HashSet<String> = exec::run("gem", &["env", "gempath"], QUICK)
        .stdout
        .trim()
        .split(':')
        .filter(|dir| dir.contains("/Cellar/"))
        .flat_map(|dir| {
            fs::read_dir(Path::new(dir).join("specifications"))
                .into_iter()
                .flatten()
                .flatten()
        })
        .filter_map(|e| {
            let name = e
                .file_name()
                .to_string_lossy()
                .trim_end_matches(".gemspec")
                .to_string();
            name.rsplit_once('-').map(|(n, _)| n.to_string())
        })
        .collect();
    let list = exec::run("gem", &["list", "--local"], QUICK);
    // A `default:` marker in the version list means the gem ships with Ruby.
    let default_only: HashSet<String> = list
        .stdout
        .lines()
        .filter(|l| {
            let inner = l.split_once('(').map(|(_, r)| r).unwrap_or_default();
            inner.starts_with("default:") && !inner.contains(',')
        })
        .filter_map(|l| l.split_whitespace().next().map(str::to_string))
        .collect();
    let out = exec::run("gem", &["outdated"], SLOW);
    let jobs: Vec<(String, String, String)> = out
        .stdout
        .lines()
        .filter_map(|l| {
            // Lines show the name, then the installed and newest versions.
            let (name, rest) = l.split_once(" (")?;
            let (cur, new) = rest.trim_end_matches(')').split_once(" < ")?;
            Some((name.to_string(), cur.to_string(), new.to_string()))
        })
        .filter(|(name, _, _)| !default_only.contains(name) && !bundled.contains(name))
        .collect();
    found.items = par_map(jobs, 6, |(name, current, latest)| {
        let versions: Vec<GemVersion> =
            http::get_json(&format!("https://rubygems.org/api/v1/versions/{name}.json"))
                .unwrap_or_default();
        let date = |v: &str| {
            versions
                .iter()
                .find(|g| g.number == v && !g.prerelease)
                .and_then(|g| dates::parse(&g.created_at))
        };
        let summary = versions
            .first()
            .and_then(|g| g.summary.clone())
            .unwrap_or_default();
        Item::new(
            Source::Langs,
            format!("gem:{name}"),
            &name,
            cmd("gem", &["update", &name]),
        )
        .path(gem_bin.display().to_string())
        .description(summary)
        .versions(
            Release::new(&current, date(&current)),
            Release::new(&latest, date(&latest)),
        )
    });
    found
}

// ---------- managers that update themselves ----------

struct SelfUpdater {
    bin: &'static str,
    repo: (&'static str, &'static str),
    /// Text before the version in release tags, such as `bun-v`.
    tag_prefix: &'static str,
    args: &'static [&'static str],
    description: &'static str,
}

const SELF_UPDATERS: &[SelfUpdater] = &[
    SelfUpdater {
        bin: "uv",
        repo: ("astral-sh", "uv"),
        tag_prefix: "",
        args: &["self", "update"],
        description: copy::DESC_UV,
    },
    SelfUpdater {
        bin: "bun",
        repo: ("oven-sh", "bun"),
        tag_prefix: "bun-v",
        args: &["upgrade"],
        description: copy::DESC_BUN,
    },
    SelfUpdater {
        bin: "deno",
        repo: ("denoland", "deno"),
        tag_prefix: "v",
        args: &["upgrade"],
        description: copy::DESC_DENO,
    },
];

#[derive(Deserialize)]
struct GhLatest {
    tag_name: String,
    published_at: Option<String>,
}

/// Standalone uv, Bun, and Deno installs. This skips Homebrew installs,
/// because Homebrew updates those.
fn self_updaters() -> Discovery {
    let mut found = Discovery::default();
    let jobs: Vec<&SelfUpdater> = SELF_UPDATERS
        .iter()
        .filter(|s| exec::which(s.bin).is_some_and(|p| origin::of(&p) == Origin::Native))
        .collect();
    found.items = par_map(jobs, 3, |s| {
        let path = exec::which(s.bin)?;
        let out = exec::run(s.bin, &["--version"], QUICK);
        let current = version::find_in(&out.stdout)?;
        let (owner, repo) = s.repo;
        let latest: GhLatest =
            http::github(&format!("repos/{owner}/{repo}/releases/latest")).ok()?;
        let latest_ver = latest.tag_name.trim_start_matches(s.tag_prefix).to_string();
        if !version::is_newer(&latest_ver, &current) {
            return None;
        }
        let cur_date = dates::github_tag_date(owner, repo, &format!("{}{current}", s.tag_prefix));
        Some(
            Item::new(
                Source::Langs,
                format!("self:{}", s.bin),
                s.bin,
                Action::Command {
                    program: path.display().to_string(),
                    args: s.args.iter().map(|a| a.to_string()).collect(),
                    sudo: false,
                },
            )
            .path(path.display().to_string())
            .description(s.description)
            .versions(
                Release::new(current, cur_date),
                Release::new(
                    latest_ver,
                    latest.published_at.as_deref().and_then(dates::parse),
                ),
            ),
        )
    })
    .into_iter()
    .flatten()
    .collect();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_specs() {
        assert!(is_local_spec("/Users/me/src/tool"));
        assert!(is_local_spec("git+https://github.com/x/y"));
        assert!(!is_local_spec("black"));
    }
}
