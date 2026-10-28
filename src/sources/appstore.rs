//! Mac App Store apps, opt-in. Finding apps, checking versions, and running
//! updates follow the approach of mas, https://github.com/mas-cli/mas, without
//! needing mas itself. mas carries this notice: Copyright 2015 Andrew Naylor,
//! Ross Goldberg. Released under the MIT License.

use std::time::Duration;

use serde::Deserialize;

use super::apps::{AppInfo, installed_apps};
use super::{Ctx, par_map};
use crate::copy;
use crate::dates;
use crate::exec;
use crate::http;
use crate::model::{Action, Discovery, Item, Release, Source};
use crate::version;

#[derive(Deserialize)]
struct Lookup {
    #[serde(default)]
    results: Vec<StoreApp>,
}

#[derive(Deserialize)]
struct StoreApp {
    #[serde(rename = "trackId")]
    track_id: u64,
    version: String,
    #[serde(rename = "currentVersionReleaseDate")]
    released: Option<String>,
    #[serde(rename = "minimumOsVersion")]
    min_os: Option<String>,
    description: Option<String>,
}

pub fn discover(ctx: &Ctx) -> Discovery {
    let mut found = Discovery::default();
    let apps: Vec<AppInfo> = installed_apps(ctx).into_iter().filter(|a| a.mas).collect();
    if apps.is_empty() {
        return found;
    }
    let country = storefront_country();
    let os = exec::run("sw_vers", &["-productVersion"], Duration::from_secs(5))
        .stdout
        .trim()
        .to_string();
    found.items = par_map(apps, 6, |app| check(&app, &country, &os))
        .into_iter()
        .flatten()
        .collect();
    found
}

fn check(app: &AppInfo, country: &str, os: &str) -> Option<Item> {
    let store = lookup(&app.bundle_id, country)?;
    if store
        .min_os
        .as_deref()
        .is_some_and(|min| version::is_newer(min, os))
    {
        return None;
    }
    if !version::is_newer(&store.version, &app.version) {
        return None;
    }
    let adam_id = adam_id(app).unwrap_or(store.track_id);
    let description = store
        .description
        .as_deref()
        .and_then(|d| d.lines().map(str::trim).find(|l| !l.is_empty()))
        .map(|l| l.chars().take(160).collect::<String>())
        .unwrap_or_default();
    Some(
        Item::new(
            Source::AppStore,
            format!("appstore:{}", app.name),
            &app.name,
            Action::AppStore {
                adam_id,
                app: app.path.display().to_string(),
            },
        )
        .path(app.path.display().to_string())
        .description(description)
        .versions(
            Release::new(&app.version, None),
            Release::new(
                &store.version,
                store.released.as_deref().and_then(dates::parse),
            ),
        ),
    )
}

/// Asks Apple's lookup API about an app. Apple caches answers for hours, so
/// a changing query value asks for a fresh one.
fn lookup(bundle_id: &str, country: &str) -> Option<StoreApp> {
    let bust = jiff::Timestamp::now().as_second() / 60;
    for entity in ["&entity=desktopSoftware", ""] {
        let url = format!(
            "https://itunes.apple.com/lookup?bundleId={bundle_id}{entity}&country={country}&t={bust}"
        );
        if let Ok(reply) = http::get_json::<Lookup>(&url)
            && let Some(app) = reply.results.into_iter().next()
        {
            return Some(app);
        }
    }
    None
}

/// The App Store item ID that Spotlight records for the app.
fn adam_id(app: &AppInfo) -> Option<u64> {
    let out = exec::run(
        "mdls",
        &[
            "-raw",
            "-name",
            "kMDItemAppStoreAdamID",
            &app.path.display().to_string(),
        ],
        Duration::from_secs(10),
    );
    out.stdout.trim().parse().ok().filter(|id| *id != 0)
}

/// The two-letter country of the signed-in App Store account. The App Store
/// keeps a one-entry cache of it; otherwise the Mac's region stands in.
fn storefront_country() -> String {
    let cache = exec::home()
        .join("Library/Caches/com.apple.appstoreagent/storefront-to-country-code.plist");
    if let Ok(plist::Value::Array(list)) = plist::Value::from_file(cache)
        && list.len() == 2
        && let Some(cc) = list[1].as_string()
    {
        return cc.to_lowercase();
    }
    let locale = exec::run(
        "defaults",
        &["read", "-g", "AppleLocale"],
        Duration::from_secs(5),
    );
    locale
        .stdout
        .trim()
        .split(['_', '@'])
        .nth(1)
        .filter(|c| c.len() == 2)
        .unwrap_or("us")
        .to_lowercase()
}

/// Updates one App Store app through the App Store's own framework. If that
/// fails, update-all opens the App Store's Updates page so you can finish.
pub fn update(adam_id: u64, app: &str) -> anyhow::Result<()> {
    match super::commerce::update(adam_id, std::path::Path::new(app)) {
        Ok(()) => Ok(()),
        Err(e) => {
            exec::run_live(
                "open",
                &["macappstore://showUpdatesPage".to_string()],
                false,
            );
            anyhow::bail!("{e}. {}", copy::ERR_APP_STORE_OPENED)
        }
    }
}
