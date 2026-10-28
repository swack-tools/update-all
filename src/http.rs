//! HTTP helpers: one shared client, a small cache with ETags, and GitHub access.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;

use crate::exec;

const BODY_LIMIT: u64 = 512 * 1024 * 1024;

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(Duration::from_secs(120)))
            .user_agent(concat!("update-all/", env!("CARGO_PKG_VERSION")))
            .http_status_as_error(false)
            .build()
            .into()
    })
}

pub fn cache_dir() -> PathBuf {
    exec::home().join("Library/Caches/update-all")
}

fn check(status: u16, url: &str) -> Result<()> {
    if (200..300).contains(&status) {
        Ok(())
    } else {
        bail!(crate::copy::fill(
            crate::copy::ERR_HTTP,
            &[&status.to_string(), url]
        ))
    }
}

pub fn get_text(url: &str) -> Result<String> {
    let mut resp = agent().get(url).call().with_context(|| url.to_string())?;
    check(resp.status().as_u16(), url)?;
    Ok(resp
        .body_mut()
        .with_config()
        .limit(BODY_LIMIT)
        .read_to_string()?)
}

pub fn get_json<T: DeserializeOwned>(url: &str) -> Result<T> {
    Ok(serde_json::from_str(&get_text(url)?)?)
}

/// Sends a JSON body and parses the JSON reply.
pub fn post_json<T: DeserializeOwned>(
    url: &str,
    headers: &[(&str, &str)],
    body: &serde_json::Value,
) -> Result<T> {
    let mut req = agent().post(url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let mut resp = req.send_json(body).with_context(|| url.to_string())?;
    check(resp.status().as_u16(), url)?;
    let text = resp
        .body_mut()
        .with_config()
        .limit(BODY_LIMIT)
        .read_to_string()?;
    Ok(serde_json::from_str(&text)?)
}

/// Fetches a URL through the cache. A copy younger than `max_age` comes from
/// disk; an older copy is revalidated with its ETag.
pub fn get_cached(url: &str, max_age: Duration) -> Result<String> {
    let dir = cache_dir();
    fs::create_dir_all(&dir).ok();
    let key = format!("{:016x}", fnv1a(url));
    let body_path = dir.join(format!("{key}.body"));
    let etag_path = dir.join(format!("{key}.etag"));

    if fresh(&body_path, max_age)
        && let Ok(body) = fs::read_to_string(&body_path)
    {
        return Ok(body);
    }

    let mut req = agent().get(url);
    let etag = fs::read_to_string(&etag_path).ok();
    if body_path.exists()
        && let Some(tag) = etag.as_deref()
    {
        req = req.header("If-None-Match", tag.trim());
    }
    let mut resp = match req.call() {
        Ok(r) => r,
        // Offline: an old copy beats nothing.
        Err(e) => return fs::read_to_string(&body_path).map_err(|_| e.into()),
    };
    if resp.status().as_u16() == 304 {
        touch(&body_path);
        return Ok(fs::read_to_string(&body_path)?);
    }
    check(resp.status().as_u16(), url)?;
    let new_tag = resp
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body = resp
        .body_mut()
        .with_config()
        .limit(BODY_LIMIT)
        .read_to_string()?;
    let _ = fs::write(&body_path, &body);
    match new_tag {
        Some(tag) => {
            let _ = fs::write(&etag_path, tag);
        }
        None => {
            let _ = fs::remove_file(&etag_path);
        }
    }
    Ok(body)
}

fn fresh(path: &Path, max_age: Duration) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < max_age)
}

fn touch(path: &Path) {
    if let Ok(file) = fs::File::options().append(true).open(path) {
        let _ = file.set_modified(SystemTime::now());
    }
}

fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Saves a URL to a file and returns the number of bytes written.
pub fn download(url: &str, dest: &Path) -> Result<u64> {
    let mut resp = agent()
        .get(url)
        .config()
        .timeout_global(Some(Duration::from_secs(1800)))
        .build()
        .call()
        .with_context(|| url.to_string())?;
    check(resp.status().as_u16(), url)?;
    let mut file = fs::File::create(dest)?;
    let mut reader = resp.body_mut().with_config().limit(u64::MAX).reader();
    Ok(io::copy(&mut reader, &mut file)?)
}

/// A GitHub token from GITHUB_TOKEN, GH_TOKEN, or `gh auth token`. It raises
/// the API rate limit from 60 to 5,000 requests an hour.
fn github_token() -> Option<&'static str> {
    static TOKEN: OnceLock<Option<String>> = OnceLock::new();
    TOKEN
        .get_or_init(|| {
            std::env::var("GITHUB_TOKEN")
                .or_else(|_| std::env::var("GH_TOKEN"))
                .ok()
                .or_else(|| {
                    let out = exec::run("gh", &["auth", "token"], Duration::from_secs(5));
                    out.ok().then(|| out.stdout.trim().to_string())
                })
                .filter(|t| !t.is_empty())
        })
        .as_deref()
}

/// Calls the GitHub API with a path such as `repos/openai/codex/releases`.
pub fn github<T: DeserializeOwned>(path: &str) -> Result<T> {
    let url = format!("https://api.github.com/{}", path.trim_start_matches('/'));
    let mut req = agent()
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");
    if let Some(token) = github_token() {
        req = req.header("Authorization", &format!("Bearer {token}"));
    }
    let mut resp = req.call().with_context(|| url.clone())?;
    check(resp.status().as_u16(), &url)?;
    let text = resp
        .body_mut()
        .with_config()
        .limit(BODY_LIMIT)
        .read_to_string()?;
    Ok(serde_json::from_str(&text)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_keys_are_stable() {
        assert_eq!(fnv1a("https://example.com"), fnv1a("https://example.com"));
        assert_ne!(fnv1a("https://a.example"), fnv1a("https://b.example"));
    }
}
