//! Homebrew formulae and casks.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use super::apps::AppInfo;
use super::{Ctx, catalog, par_map};
use crate::copy::{self, fill};
use crate::dates;
use crate::exec;
use crate::model::{Action, Discovery, Item, Release, Source};
use crate::version;

#[derive(Deserialize)]
struct Outdated {
    #[serde(default)]
    formulae: Vec<OutdatedEntry>,
    #[serde(default)]
    casks: Vec<OutdatedEntry>,
}

#[derive(Deserialize)]
struct OutdatedEntry {
    name: String,
    #[serde(default)]
    installed_versions: Vec<String>,
    current_version: String,
    #[serde(default)]
    pinned: bool,
}

#[derive(Deserialize, Default)]
struct Info {
    #[serde(default)]
    formulae: Vec<FormulaInfo>,
    #[serde(default)]
    casks: Vec<CaskInfo>,
}

#[derive(Deserialize)]
struct FormulaInfo {
    name: String,
    desc: Option<String>,
    homepage: Option<String>,
    urls: Option<Urls>,
}

#[derive(Deserialize)]
struct Urls {
    stable: Option<UrlEntry>,
}

#[derive(Deserialize)]
struct UrlEntry {
    url: Option<String>,
    tag: Option<String>,
}

#[derive(Deserialize)]
struct CaskInfo {
    token: String,
    desc: Option<String>,
    homepage: Option<String>,
    url: Option<String>,
    #[serde(default)]
    artifacts: Vec<serde_json::Value>,
    caveats: Option<String>,
    auto_updates: Option<bool>,
}

pub fn discover(ctx: &Ctx) -> Discovery {
    let mut found = Discovery::default();
    if !exec::has("brew") {
        return found;
    }
    // Refreshes Homebrew's package lists. This changes no installed software.
    let update = exec::run("brew", &["update", "--quiet"], Duration::from_secs(300));
    if !update.ok() {
        found.notes.push(copy::NOTE_BREW_UPDATE_FAILED.to_string());
    }
    let out = exec::run(
        "brew",
        &["outdated", "--json=v2", "--greedy"],
        Duration::from_secs(180),
    );
    let Ok(outdated) = serde_json::from_str::<Outdated>(&out.stdout) else {
        found
            .notes
            .push(copy::NOTE_BREW_OUTDATED_FAILED.to_string());
        return found;
    };

    let info = brew_info(&outdated);
    found
        .items
        .extend(formulae(&outdated.formulae, &info.formulae));
    let (casks, notes) = casks(ctx, &outdated.casks, &info.casks);
    found.items.extend(casks);
    found.notes.extend(notes);
    found
}

fn brew_info(outdated: &Outdated) -> Info {
    let mut info = Info::default();
    let get = |flag: &str, names: Vec<&str>| -> Info {
        if names.is_empty() {
            return Info::default();
        }
        let mut args = vec!["info", "--json=v2", flag];
        args.extend(names);
        let out = exec::run("brew", &args, Duration::from_secs(120));
        serde_json::from_str(&out.stdout).unwrap_or_default()
    };
    info.formulae = get(
        "--formula",
        outdated.formulae.iter().map(|f| f.name.as_str()).collect(),
    )
    .formulae;
    info.casks = get(
        "--cask",
        outdated.casks.iter().map(|c| c.name.as_str()).collect(),
    )
    .casks;
    info
}

fn formulae(entries: &[OutdatedEntry], info: &[FormulaInfo]) -> Vec<Item> {
    let prefix = brew_prefix();
    let jobs: Vec<(&OutdatedEntry, Option<&FormulaInfo>)> = entries
        .iter()
        .map(|e| (e, info.iter().find(|i| i.name == e.name)))
        .collect();
    par_map(jobs, 8, |(entry, info)| {
        let current = entry.installed_versions.last().cloned().unwrap_or_default();
        let latest = entry.current_version.clone();
        let (cur_date, new_date) = info
            .and_then(|i| formula_dates(i, &current, &latest))
            .unwrap_or((None, None));
        let mut item = Item::new(
            Source::Brew,
            format!("brew:{}", entry.name),
            &entry.name,
            Action::BrewFormula(entry.name.clone()),
        )
        .path(prefix.join("opt").join(&entry.name).display().to_string())
        .description(info.and_then(|i| i.desc.clone()).unwrap_or_default())
        .versions(
            Release::new(current, cur_date),
            Release::new(latest, new_date),
        );
        if entry.pinned {
            item.held = true;
            item.note = Some(copy::NOTE_BREW_PINNED.to_string());
        }
        item
    })
}

/// Release dates from GitHub when the formula's source lives there.
fn formula_dates(
    info: &FormulaInfo,
    current: &str,
    latest: &str,
) -> Option<(Option<jiff::Timestamp>, Option<jiff::Timestamp>)> {
    let stable = info.urls.as_ref()?.stable.as_ref()?;
    let url = stable.url.as_deref().unwrap_or_default();
    let (owner, repo) = dates::github_repo(url)
        .or_else(|| info.homepage.as_deref().and_then(dates::github_repo))?;
    let tag = stable
        .tag
        .clone()
        .or_else(|| dates::github_tag(url))
        .unwrap_or_else(|| format!("v{latest}"));
    // Homebrew adds `_1` style revisions that upstream tags never have.
    let strip = |v: &str| v.split('_').next().unwrap_or(v).to_string();
    Some(dates::github_pair(
        &owner,
        &repo,
        &tag,
        &strip(latest),
        &strip(current),
    ))
}

fn casks(ctx: &Ctx, entries: &[OutdatedEntry], info: &[CaskInfo]) -> (Vec<Item>, Vec<String>) {
    let mut notes = Vec::new();
    let mut jobs = Vec::new();
    let installed = super::apps::installed_apps(ctx);
    for entry in entries {
        let info = info.iter().find(|i| i.token == entry.name);
        let artifacts = info.map(|i| i.artifacts.as_slice()).unwrap_or_default();
        let app_names = catalog::app_artifacts(artifacts);
        let app = app_names
            .iter()
            .find_map(|name| locate_app(name))
            .or_else(|| {
                // A pkg cask names its app only by bundle ID.
                let ids = catalog::quit_ids(artifacts);
                installed
                    .iter()
                    .find(|a| ids.contains(&a.bundle_id))
                    .map(|a| a.path.clone())
            });
        if !app_names.is_empty() && app.is_none() {
            // Upgrading would install an app that isn't there anymore.
            notes.push(fill(
                copy::NOTE_CASK_APP_MISSING,
                &[&entry.name, &app_names[0]],
            ));
            continue;
        }
        let app_info = app.as_deref().and_then(AppInfo::read);
        if let Some(app_path) = &app
            && ctx.config.ignored(app_path)
        {
            continue;
        }
        let recorded = entry.installed_versions.last().cloned().unwrap_or_default();
        let latest = entry.current_version.clone();
        // Apps that update themselves often run ahead of Homebrew's record.
        let current = app_info
            .as_ref()
            .map(|a| a.version.clone())
            .filter(|v| !v.is_empty())
            .unwrap_or(recorded);
        if app_info.is_some() && !version::is_newer(&latest, &current) {
            continue;
        }
        jobs.push((entry, info, app, app_info, current, latest));
    }
    let items = par_map(jobs, 8, |(entry, info, app, app_info, current, latest)| {
        let url = info.and_then(|i| i.url.clone()).unwrap_or_default();
        let (cur_date, new_date) = match dates::github_repo(&url) {
            Some((owner, repo)) => {
                let tag = dates::github_tag(&url).unwrap_or_else(|| format!("v{latest}"));
                dates::github_pair(
                    &owner,
                    &repo,
                    &tag,
                    version::clean(&latest),
                    version::clean(&current),
                )
            }
            None => (None, None),
        };
        let path = app.map(|p| p.display().to_string()).unwrap_or_else(|| {
            brew_prefix()
                .join("Caskroom")
                .join(&entry.name)
                .display()
                .to_string()
        });
        let mut item = Item::new(
            Source::Brew,
            format!("cask:{}", entry.name),
            app_info
                .as_ref()
                .map(|a| a.name.clone())
                .unwrap_or_else(|| entry.name.clone()),
            Action::BrewCask(entry.name.clone()),
        )
        .path(path)
        .description(
            info.and_then(|i| i.desc.clone().or_else(|| i.homepage.clone()))
                .unwrap_or_default(),
        )
        .versions(
            Release::new(version::clean(&current), cur_date),
            Release::new(version::clean(&latest), new_date),
        );
        item.electron = app_info.as_ref().is_some_and(|a| a.electron);
        item.needs_restart = info
            .and_then(|i| i.caveats.as_deref())
            .is_some_and(catalog::caveats_need_reboot);
        if info.and_then(|i| i.auto_updates).unwrap_or(false) {
            item.note = Some(copy::NOTE_CASK_SELF_UPDATES.to_string());
        }
        if entry.pinned {
            item.held = true;
            item.note = Some(copy::NOTE_BREW_PINNED.to_string());
        }
        item
    });
    (items, notes)
}

fn brew_prefix() -> PathBuf {
    if Path::new("/opt/homebrew").exists() {
        PathBuf::from("/opt/homebrew")
    } else {
        PathBuf::from("/usr/local")
    }
}

/// Finds an installed app by bundle name in the usual folders.
pub fn locate_app(name: &str) -> Option<PathBuf> {
    [
        PathBuf::from("/Applications"),
        exec::home().join("Applications"),
    ]
    .into_iter()
    .map(|dir| dir.join(name))
    .find(|p| p.exists())
}

/// The arguments for one `brew upgrade` call.
pub fn upgrade_args(kind: &str, names: &[String]) -> Vec<String> {
    let mut args = vec!["upgrade".to_string(), kind.to_string()];
    if kind == "--cask" {
        // Needed for casks marked as updating themselves.
        args.push("--greedy".to_string());
    }
    args.extend(names.iter().cloned());
    args
}

/// Upgrades formulae or casks in one Homebrew call. Returns the names that
/// are still outdated afterward, which means they failed.
pub fn upgrade(kind: &str, names: &[String]) -> Vec<String> {
    if names.is_empty() {
        return Vec::new();
    }
    exec::run_live("brew", &upgrade_args(kind, names), false);
    let out = exec::run(
        "brew",
        &["outdated", "--json=v2", "--greedy"],
        Duration::from_secs(180),
    );
    let still: Vec<String> = serde_json::from_str::<Outdated>(&out.stdout)
        .map(|o| {
            let list = if kind == "--cask" {
                o.casks
            } else {
                o.formulae
            };
            list.into_iter().map(|e| e.name).collect()
        })
        .unwrap_or_default();
    names
        .iter()
        .filter(|n| still.contains(n))
        .cloned()
        .collect()
}
