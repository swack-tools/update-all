//! Release-date parsing and lookups shared by several sources.

use jiff::Timestamp;
use serde::Deserialize;

use crate::http;

/// Parses the date formats that registries and feeds use: timestamps like
/// 2026-10-13T12:00:00Z, email-style dates like Sparkle's `pubDate`, plain
/// dates, and Unix seconds.
pub fn parse(text: &str) -> Option<Timestamp> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(ts) = text.parse::<Timestamp>() {
        return Some(ts);
    }
    if let Ok(zoned) = jiff::fmt::rfc2822::parse(text) {
        return Some(zoned.timestamp());
    }
    // Some feeds write `2024-01-31 10:00:00 +0000` or skip the zone.
    if let Ok(dt) = text.replace(' ', "T").parse::<jiff::civil::DateTime>() {
        return dt
            .to_zoned(jiff::tz::TimeZone::UTC)
            .ok()
            .map(|z| z.timestamp());
    }
    if let Some(date) = text
        .get(..10)
        .and_then(|d| d.parse::<jiff::civil::Date>().ok())
    {
        return date
            .to_zoned(jiff::tz::TimeZone::UTC)
            .ok()
            .map(|z| z.timestamp());
    }
    if let Ok(secs) = text.parse::<i64>() {
        return Timestamp::from_second(secs).ok();
    }
    None
}

/// Finds `owner/repo` in a GitHub URL.
pub fn github_repo(url: &str) -> Option<(String, String)> {
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
        .or_else(|| url.strip_prefix("git+https://github.com/"))?;
    let mut parts = rest.split('/');
    let owner = parts.next()?.to_string();
    let repo = parts.next()?.trim_end_matches(".git").to_string();
    (!owner.is_empty() && !repo.is_empty()).then_some((owner, repo))
}

/// Finds the tag in a GitHub download URL, such as `v1.2.3` in a
/// `/releases/download/v1.2.3/x.tar.gz` or `/archive/refs/tags/v1.2.3.tar.gz` path.
pub fn github_tag(url: &str) -> Option<String> {
    if let Some((_, rest)) = url.split_once("/releases/download/") {
        return rest.split('/').next().map(str::to_string);
    }
    if let Some((_, rest)) = url.split_once("/archive/refs/tags/") {
        return Some(strip_archive(rest).to_string());
    }
    if let Some((_, rest)) = url.split_once("/archive/") {
        return Some(strip_archive(rest).to_string());
    }
    None
}

fn strip_archive(name: &str) -> &str {
    name.trim_end_matches(".tar.gz")
        .trim_end_matches(".tgz")
        .trim_end_matches(".zip")
        .trim_end_matches(".tar.xz")
}

#[derive(Deserialize)]
struct GhRelease {
    published_at: Option<String>,
}

#[derive(Deserialize)]
struct GhCommit {
    commit: GhCommitInner,
}

#[derive(Deserialize)]
struct GhCommitInner {
    committer: GhPerson,
}

#[derive(Deserialize)]
struct GhPerson {
    date: String,
}

/// The release date of a tag: the GitHub release date, or the tagged commit's date.
pub fn github_tag_date(owner: &str, repo: &str, tag: &str) -> Option<Timestamp> {
    if let Ok(rel) = http::github::<GhRelease>(&format!("repos/{owner}/{repo}/releases/tags/{tag}"))
        && let Some(date) = rel.published_at.as_deref().and_then(parse)
    {
        return Some(date);
    }
    http::github::<GhCommit>(&format!("repos/{owner}/{repo}/commits/{tag}"))
        .ok()
        .and_then(|c| parse(&c.commit.committer.date))
}

/// Dates for the installed and newest versions on GitHub. update-all guesses
/// the installed tag from the newest tag by swapping the version inside it,
/// which works for the common `v1.2.3`, `1.2.3`, and `name-v1.2.3` styles.
pub fn github_pair(
    owner: &str,
    repo: &str,
    latest_tag: &str,
    latest: &str,
    current: &str,
) -> (Option<Timestamp>, Option<Timestamp>) {
    let latest_date = github_tag_date(owner, repo, latest_tag);
    let current_tag = latest_tag.replace(latest, current);
    let current_date = if current_tag != latest_tag && latest_date.is_some() {
        github_tag_date(owner, repo, &current_tag)
    } else {
        None
    };
    (current_date, latest_date)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_formats() {
        assert!(parse("2026-10-13T12:00:00Z").is_some());
        assert!(parse("2026-10-13T12:00:00.123+02:00").is_some());
        assert!(parse("Tue, 13 Oct 2026 12:00:00 +0000").is_some());
        assert!(parse("2026-10-13").is_some());
        assert!(parse("2026-10-13 12:00:00").is_some());
        assert!(parse("1793037690").is_some());
        assert!(parse("soon").is_none());
    }

    #[test]
    fn finds_github_parts() {
        let url = "https://github.com/openai/codex/releases/download/rust-v0.165.0/codex.tar.gz";
        assert_eq!(
            github_repo(url),
            Some(("openai".to_string(), "codex".to_string()))
        );
        assert_eq!(github_tag(url).as_deref(), Some("rust-v0.165.0"));
        assert_eq!(
            github_tag("https://github.com/casey/just/archive/refs/tags/1.58.0.tar.gz").as_deref(),
            Some("1.58.0")
        );
        assert_eq!(github_repo("https://nodejs.org/dist/v26/node.tar.xz"), None);
    }
}
