//! Docker images whose tag now points to a newer image in the registry.
//! update-all compares digests without pulling, then pulls only what changed.
//! It never restarts containers.

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;

use super::{Ctx, par_map};
use crate::copy::{self, fill};
use crate::dates;
use crate::exec;
use crate::model::{Action, Discovery, Item, Release, Source};

#[derive(Deserialize)]
struct Listed {
    #[serde(rename = "Repository")]
    repository: String,
    #[serde(rename = "Tag")]
    tag: String,
}

#[derive(Deserialize)]
struct Inspected {
    #[serde(rename = "RepoDigests", default)]
    repo_digests: Vec<String>,
    #[serde(rename = "Created")]
    created: Option<String>,
}

pub fn discover(ctx: &Ctx) -> Discovery {
    let mut found = Discovery::default();
    if !exec::has("docker") {
        return found;
    }
    let ping = exec::run(
        "docker",
        &["info", "--format", "{{.ServerVersion}}"],
        Duration::from_secs(15),
    );
    if !ping.ok() {
        found.notes.push(copy::NOTE_DOCKER_DOWN.to_string());
        return found;
    }
    if !exec::run("docker", &["buildx", "version"], Duration::from_secs(10)).ok() {
        found.notes.push(copy::NOTE_DOCKER_NO_BUILDX.to_string());
        return found;
    }
    let out = exec::run(
        "docker",
        &["images", "--format", "{{json .}}"],
        Duration::from_secs(60),
    );
    let mut refs: Vec<String> = out
        .stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<Listed>(l).ok())
        .filter(|i| i.repository != "<none>" && i.tag != "<none>")
        .map(|i| format!("{}:{}", i.repository, i.tag))
        .collect();
    refs.sort();
    refs.dedup();
    refs.retain(|r| !ctx.config.skips(&format!("docker:{r}"), r));
    let limited = Mutex::new(HashSet::new());
    let (mut rate_limited, mut failed) = (0, 0);
    for checked in par_map(refs, 6, |r| check(r, &limited)) {
        match checked {
            Checked::Newer(item) => found.items.push(*item),
            Checked::Current => {}
            Checked::RateLimited => rate_limited += 1,
            Checked::Failed => failed += 1,
        }
    }
    found.incomplete = rate_limited + failed > 0;
    if rate_limited > 0 {
        found.notes.push(fill(
            copy::NOTE_DOCKER_RATE_LIMIT,
            &[&rate_limited.to_string()],
        ));
    }
    if failed > 0 {
        found
            .notes
            .push(fill(copy::NOTE_DOCKER_FAILED, &[&failed.to_string()]));
    }
    found
}

enum Checked {
    Newer(Box<Item>),
    Current,
    RateLimited,
    Failed,
}

/// The registry that serves an image: the first path part when it looks
/// like a host name, otherwise Docker Hub.
pub fn registry(reference: &str) -> &str {
    match reference.split_once('/') {
        Some((host, _)) if host.contains('.') || host.contains(':') || host == "localhost" => host,
        _ => "docker.io",
    }
}

fn check(reference: String, limited: &Mutex<HashSet<String>>) -> Checked {
    let host = registry(&reference).to_string();
    let long = Duration::from_secs(60);
    let local = exec::run("docker", &["image", "inspect", &reference], long);
    let Some(local) = serde_json::from_str::<Vec<Inspected>>(&local.stdout)
        .ok()
        .and_then(|l| l.into_iter().next())
    else {
        return Checked::Current;
    };
    let digests: Vec<&str> = local
        .repo_digests
        .iter()
        .filter_map(|d| d.split_once('@').map(|(_, h)| h))
        .collect();
    // Images built on this Mac have no registry digest.
    if digests.is_empty() {
        return Checked::Current;
    }
    // Once a registry refuses requests, more of them only count against its
    // limit, so the remaining checks for that registry stop.
    if limited.lock().is_ok_and(|l| l.contains(&host)) {
        return Checked::RateLimited;
    }
    let remote = exec::run(
        "docker",
        &[
            "buildx",
            "imagetools",
            "inspect",
            "--format",
            "{{.Manifest.Digest}}",
            &reference,
        ],
        long,
    );
    if !remote.ok() {
        let err = remote.stderr.to_lowercase();
        if err.contains("429") || err.contains("toomanyrequests") {
            if let Ok(mut l) = limited.lock() {
                l.insert(host);
            }
            return Checked::RateLimited;
        }
        // Local or private images that the registry doesn't know count as current.
        if ["not found", "unauthorized", "denied", "401", "403", "404"]
            .iter()
            .any(|m| err.contains(m))
        {
            return Checked::Current;
        }
        return Checked::Failed;
    }
    let remote_digest = remote.stdout.trim().to_string();
    if remote_digest.is_empty() || digests.contains(&remote_digest.as_str()) {
        return Checked::Current;
    }
    let remote_created = remote_created(&reference);
    let short = |d: &str| {
        d.trim_start_matches("sha256:")
            .chars()
            .take(12)
            .collect::<String>()
    };
    Checked::Newer(Box::new(
        Item::new(
            Source::Docker,
            format!("docker:{reference}"),
            &reference,
            Action::DockerPull(reference.clone()),
        )
        .path(&reference)
        .description(copy::DESC_DOCKER)
        .versions(
            Release::new(
                short(digests[0]),
                local.created.as_deref().and_then(dates::parse),
            ),
            Release::new(short(&remote_digest), remote_created),
        ),
    ))
}

/// How a pull went.
pub enum Pull {
    Done,
    RateLimited,
    Failed,
}

/// Pulls one tag and prints Docker's output. update-all reads the output
/// first, so a rate limit can stop the remaining pulls from the same registry.
pub fn pull(reference: &str) -> Pull {
    let out = exec::run("docker", &["pull", reference], Duration::from_secs(3600));
    for line in out.stdout.lines().chain(out.stderr.lines()) {
        println!("{line}");
    }
    classify(out.ok(), &out.stderr)
}

fn classify(ok: bool, stderr: &str) -> Pull {
    let err = stderr.to_lowercase();
    if ok {
        Pull::Done
    } else if err.contains("429") || err.contains("toomanyrequests") {
        Pull::RateLimited
    } else {
        Pull::Failed
    }
}

/// The registry image's creation date, for this Mac's platform when the tag
/// covers several platforms.
fn remote_created(reference: &str) -> Option<jiff::Timestamp> {
    let out = exec::run(
        "docker",
        &[
            "buildx",
            "imagetools",
            "inspect",
            "--format",
            "{{json .Image}}",
            reference,
        ],
        Duration::from_secs(60),
    );
    let v: serde_json::Value = serde_json::from_str(&out.stdout).ok()?;
    let created = v
        .get("created")
        .or_else(|| v.get("linux/arm64").and_then(|p| p.get("created")))
        .or_else(|| v.as_object()?.values().find_map(|p| p.get("created")))?;
    dates::parse(created.as_str()?)
}

#[cfg(test)]
mod tests {
    use super::{Pull, classify, registry};

    #[test]
    fn spots_rate_limits() {
        let err = "Error response from daemon: unknown: failed to resolve reference \
\"docker.io/library/alpine:3.23\": unexpected status from HEAD request to \
https://registry-1.docker.io/v2/library/alpine/manifests/3.23: 429 Too Many Requests";
        assert!(matches!(classify(false, err), Pull::RateLimited));
        assert!(matches!(classify(false, "manifest unknown"), Pull::Failed));
        assert!(matches!(classify(true, ""), Pull::Done));
    }

    #[test]
    fn finds_registries() {
        assert_eq!(registry("alpine:3.23"), "docker.io");
        assert_eq!(registry("dunglas/frankenphp:1"), "docker.io");
        assert_eq!(
            registry("lscr.io/linuxserver/baseimage-alpine:3.23"),
            "lscr.io"
        );
        assert_eq!(registry("localhost:5000/app:dev"), "localhost:5000");
    }
}
