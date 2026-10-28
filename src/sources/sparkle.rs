//! A Sparkle-compatible updater. It reads an app's appcast, checks the
//! download's EdDSA signature against the public key inside the app, checks
//! that the new app comes from the same developer, and swaps it in only while
//! the app isn't running.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};

use crate::copy::{self, fill};
use crate::dates;
use crate::exec;
use crate::http;
use crate::version;

const SPARKLE_NS: &str = "http://www.andymatuschak.org/xml-namespaces/sparkle";

/// One release in an appcast.
#[derive(Clone, Debug, Default)]
pub struct FeedItem {
    /// The bundle version, CFBundleVersion, which Sparkle compares.
    pub build: String,
    /// The version people see, CFBundleShortVersionString.
    pub display: String,
    pub date: Option<jiff::Timestamp>,
    pub url: String,
    pub length: Option<u64>,
    pub ed_signature: Option<String>,
}

/// Everything needed to install one Sparkle update.
#[derive(Clone, Debug)]
pub struct Update {
    pub app: PathBuf,
    pub bundle_id: String,
    pub item: FeedItem,
    pub public_key: String,
}

/// Parses an appcast and keeps the releases this Mac can run on the default
/// channel, newest first.
pub fn parse_feed(xml: &str, os_version: &str) -> Vec<FeedItem> {
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return Vec::new();
    };
    let mut items = Vec::new();
    for node in doc.descendants().filter(|n| n.has_tag_name("item")) {
        let child = |name: &str| {
            node.children()
                .find(|c| c.has_tag_name((SPARKLE_NS, name)))
                .and_then(|c| c.text())
                .map(|t| t.trim().to_string())
        };
        // Beta and other named channels are opt-in inside the app itself.
        // Some feeds label their main channel `stable` or `release`.
        if child("channel").is_some_and(|c| !matches!(c.as_str(), "stable" | "release" | "default"))
        {
            continue;
        }
        let Some(enc) = node.children().find(|c| c.has_tag_name("enclosure")) else {
            continue;
        };
        if enc
            .attribute((SPARKLE_NS, "os"))
            .is_some_and(|os| !os.eq_ignore_ascii_case("macos"))
        {
            continue;
        }
        // Sparkle prefers the enclosure attribute when both forms exist.
        let build = enc
            .attribute((SPARKLE_NS, "version"))
            .map(str::to_string)
            .or_else(|| child("version"))
            .unwrap_or_default();
        let display = enc
            .attribute((SPARKLE_NS, "shortVersionString"))
            .map(str::to_string)
            .or_else(|| child("shortVersionString"))
            .unwrap_or_else(|| build.clone());
        if child("minimumSystemVersion").is_some_and(|min| version::is_newer(&min, os_version)) {
            continue;
        }
        let Some(url) = enc.attribute("url") else {
            continue;
        };
        if build.is_empty() {
            continue;
        }
        let date = node
            .children()
            .find(|c| c.has_tag_name("pubDate"))
            .and_then(|c| c.text())
            .and_then(dates::parse);
        items.push(FeedItem {
            build,
            display,
            date,
            url: url.to_string(),
            length: enc.attribute("length").and_then(|l| l.parse().ok()),
            ed_signature: enc
                .attribute((SPARKLE_NS, "edSignature"))
                .map(str::to_string),
        });
    }
    items.sort_by(|a, b| version::compare(&b.build, &a.build));
    items
}

pub fn fetch_feed(url: &str, os_version: &str) -> Vec<FeedItem> {
    http::get_cached(url, Duration::from_secs(1800))
        .map(|xml| parse_feed(&xml, os_version))
        .unwrap_or_default()
}

/// Checks a Sparkle EdDSA signature. Sparkle signs the raw archive bytes
/// with ed25519, and the app carries the base64 public key as `SUPublicEDKey`.
pub fn verify_signature(data: &[u8], signature_b64: &str, public_key_b64: &str) -> Result<()> {
    let key: [u8; 32] = B64
        .decode(public_key_b64.trim())?
        .try_into()
        .map_err(|_| anyhow!(copy::ERR_SPARKLE_KEY))?;
    let sig: [u8; 64] = B64
        .decode(signature_b64.trim())?
        .try_into()
        .map_err(|_| anyhow!(copy::ERR_SPARKLE_SIG_SIZE))?;
    VerifyingKey::from_bytes(&key)?
        .verify(data, &Signature::from_bytes(&sig))
        .map_err(|_| anyhow!(copy::ERR_SPARKLE_SIG))
}

/// True when the app has a process running from inside its bundle.
pub fn is_running(app: &Path) -> bool {
    let pattern = format!("{}/Contents/MacOS/", app.display());
    exec::run("pgrep", &["-f", &pattern], Duration::from_secs(5)).ok()
}

/// The code-signing Team ID, such as `EQHXZ8M8AV`.
pub fn team_id(app: &Path) -> Option<String> {
    let out = exec::run(
        "codesign",
        &["-dv", "--verbose=2", &app.display().to_string()],
        Duration::from_secs(30),
    );
    // codesign prints its details to stderr.
    out.stderr
        .lines()
        .find_map(|l| l.strip_prefix("TeamIdentifier="))
        .map(|t| t.trim().to_string())
        .filter(|t| t != "not set")
}

/// Downloads, verifies, and installs an update. Fails without touching the
/// installed app if any check fails.
pub fn install(update: &Update) -> Result<()> {
    let app = &update.app;
    if is_running(app) {
        bail!(copy::ERR_APP_RUNNING);
    }
    let sig = update
        .item
        .ed_signature
        .as_deref()
        .ok_or_else(|| anyhow!(copy::ERR_NO_SIGNATURE))?;
    let work = http::cache_dir()
        .join("sparkle")
        .join(format!("{}-{}", update.bundle_id, update.item.build));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work)?;
    let result = install_in(update, sig, &work);
    let _ = fs::remove_dir_all(&work);
    result
}

fn install_in(update: &Update, sig: &str, work: &Path) -> Result<()> {
    let app = &update.app;
    let name = update
        .item
        .url
        .rsplit('/')
        .next()
        .unwrap_or("update")
        .to_string();
    let name = name.split('?').next().unwrap_or("update").to_string();
    let archive = work.join(&name);
    let size = http::download(&update.item.url, &archive)?;
    if let Some(expected) = update.item.length
        && expected != 0
        && expected != size
    {
        bail!(fill(
            copy::ERR_SPARKLE_SIZE,
            &[&size.to_string(), &expected.to_string()]
        ));
    }
    let data = fs::read(&archive)?;
    verify_signature(&data, sig, &update.public_key)?;
    drop(data);

    let extracted = work.join("extracted");
    fs::create_dir_all(&extracted)?;
    extract(&archive, &extracted)?;
    let new_app = find_app(&extracted, &update.bundle_id)
        .ok_or_else(|| anyhow!(fill(copy::ERR_SPARKLE_NO_APP, &[&update.bundle_id])))?;

    let verify = exec::run(
        "codesign",
        &[
            "--verify",
            "--deep",
            "--strict",
            &new_app.display().to_string(),
        ],
        Duration::from_secs(300),
    );
    if !verify.ok() {
        bail!(fill(copy::ERR_SPARKLE_CODESIGN, &[verify.stderr.trim()]));
    }
    let (old_team, new_team) = (team_id(app), team_id(&new_app));
    if old_team.is_none() || old_team != new_team {
        let none = copy::TEAM_NONE.to_string();
        bail!(fill(
            copy::ERR_SPARKLE_TEAM,
            &[
                new_team.as_ref().unwrap_or(&none),
                old_team.as_ref().unwrap_or(&none)
            ]
        ));
    }
    if is_running(app) {
        bail!(copy::ERR_APP_RUNNING);
    }

    // Keep the old app until the new one is in place.
    let backup = work.join("previous.app");
    fs::rename(app, &backup)
        .with_context(|| fill(copy::ERR_SPARKLE_MOVE, &[&app.display().to_string()]))?;
    let copied = exec::run(
        "ditto",
        &[&new_app.display().to_string(), &app.display().to_string()],
        Duration::from_secs(600),
    );
    if !copied.ok() {
        let _ = fs::remove_dir_all(app);
        fs::rename(&backup, app)?;
        bail!(fill(copy::ERR_SPARKLE_COPY, &[copied.stderr.trim()]));
    }
    // Tell Launch Services about the new version.
    let _ = exec::run(
        "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister",
        &["-f", &app.display().to_string()],
        Duration::from_secs(30),
    );
    Ok(())
}

fn extract(archive: &Path, dest: &Path) -> Result<()> {
    let a = archive.display().to_string();
    let d = dest.display().to_string();
    let lower = a.to_lowercase();
    let long = Duration::from_secs(600);
    if lower.ends_with(".zip") {
        let out = exec::run("ditto", &["-x", "-k", &a, &d], long);
        if !out.ok() {
            bail!(fill(copy::ERR_SPARKLE_UNZIP, &[out.stderr.trim()]));
        }
    } else if lower.ends_with(".dmg") {
        let mount = dest.join("mount");
        fs::create_dir_all(&mount)?;
        let m = mount.display().to_string();
        let out = exec::run(
            "hdiutil",
            &[
                "attach",
                "-nobrowse",
                "-readonly",
                "-noautoopen",
                "-mountpoint",
                &m,
                &a,
            ],
            long,
        );
        if !out.ok() {
            bail!(fill(copy::ERR_SPARKLE_DMG, &[out.stderr.trim()]));
        }
        let copied = fs::read_dir(&mount)?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x == "app"))
            .map(|src| {
                let target = dest.join(src.file_name().unwrap_or_default());
                exec::run(
                    "ditto",
                    &[&src.display().to_string(), &target.display().to_string()],
                    long,
                )
                .ok()
            });
        let _ = exec::run(
            "hdiutil",
            &["detach", "-force", &m],
            Duration::from_secs(60),
        );
        if copied != Some(true) {
            bail!(copy::ERR_SPARKLE_DMG_EMPTY);
        }
    } else if lower.ends_with(".tar.gz")
        || lower.ends_with(".tgz")
        || lower.ends_with(".tar.xz")
        || lower.ends_with(".tar.bz2")
        || lower.ends_with(".tar")
    {
        let out = exec::run("tar", &["-xf", &a, "-C", &d], long);
        if !out.ok() {
            bail!(fill(copy::ERR_SPARKLE_UNPACK, &[out.stderr.trim()]));
        }
    } else {
        bail!(fill(
            copy::ERR_SPARKLE_KIND,
            &[&archive.display().to_string()]
        ));
    }
    Ok(())
}

/// Finds the app with the right bundle ID, up to two folders deep.
fn find_app(dir: &Path, bundle_id: &str) -> Option<PathBuf> {
    let mut stack = vec![(dir.to_path_buf(), 0)];
    while let Some((d, depth)) = stack.pop() {
        for entry in fs::read_dir(&d).ok()?.flatten() {
            let p = entry.path();
            if p.extension().is_some_and(|x| x == "app") {
                if super::apps::AppInfo::read(&p).is_some_and(|a| a.bundle_id == bundle_id) {
                    return Some(p);
                }
            } else if p.is_dir() && depth < 2 {
                stack.push((p, depth + 1));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    const FEED: &str = r#"<?xml version="1.0"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
<channel>
  <item>
    <title>2.1</title>
    <pubDate>Tue, 13 Oct 2026 12:00:00 +0000</pubDate>
    <sparkle:version>210</sparkle:version>
    <sparkle:shortVersionString>2.1</sparkle:shortVersionString>
    <sparkle:minimumSystemVersion>14.0</sparkle:minimumSystemVersion>
    <enclosure url="https://example.com/App-2.1.zip" length="100" type="application/octet-stream" sparkle:edSignature="c2ln"/>
  </item>
  <item>
    <title>2.2 beta</title>
    <sparkle:channel>beta</sparkle:channel>
    <enclosure url="https://example.com/App-2.2b.zip" sparkle:version="220"/>
  </item>
  <item>
    <title>3.0</title>
    <sparkle:minimumSystemVersion>99.0</sparkle:minimumSystemVersion>
    <enclosure url="https://example.com/App-3.0.zip" sparkle:version="300"/>
  </item>
  <item>
    <title>2.0</title>
    <pubDate>Tue, 01 Sep 2026 12:00:00 +0000</pubDate>
    <enclosure url="https://example.com/App-2.0.zip" sparkle:version="200" sparkle:shortVersionString="2.0"/>
  </item>
</channel>
</rss>"#;

    #[test]
    fn parses_feed_and_skips_betas_and_newer_os() {
        let items = parse_feed(FEED, "27.2");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].build, "210");
        assert_eq!(items[0].display, "2.1");
        assert_eq!(items[0].length, Some(100));
        assert_eq!(items[0].ed_signature.as_deref(), Some("c2ln"));
        assert!(items[0].date.is_some());
        assert_eq!(items[1].display, "2.0");
    }

    #[test]
    fn checks_signatures() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let data = b"archive bytes";
        let sig = B64.encode(key.sign(data).to_bytes());
        let public = B64.encode(key.verifying_key().to_bytes());
        assert!(verify_signature(data, &sig, &public).is_ok());
        assert!(verify_signature(b"tampered", &sig, &public).is_err());
    }
}
