//! Helm releases with newer charts. Report only: update-all prints the
//! commands and never runs them, and it never refreshes helm's repo cache.
//! When helm isn't on PATH, this source shows nothing at all.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use serde::Deserialize;

use super::{Ctx, par_map};
use crate::copy::{self, fill};
use crate::dates;
use crate::exec;
use crate::http;
use crate::model::{Age, Discovery, Release, Report, date_label};
use crate::version;

#[derive(Deserialize)]
struct Repo {
    name: String,
    url: String,
}

#[derive(Deserialize)]
struct HelmRelease {
    name: String,
    namespace: String,
    chart: String,
}

#[derive(Deserialize)]
struct Index {
    #[serde(default)]
    entries: HashMap<String, Vec<ChartVersion>>,
}

#[derive(Deserialize)]
struct ChartVersion {
    version: String,
    created: Option<String>,
    #[serde(default)]
    deprecated: bool,
}

pub fn discover(_ctx: &Ctx) -> Discovery {
    let mut found = Discovery::default();
    // Without helm on PATH, this source stays completely silent.
    if !exec::on_path("helm") {
        return found;
    }
    let out = exec::run(
        "helm",
        &["list", "--all-namespaces", "--output", "json"],
        Duration::from_secs(20),
    );
    if !out.ok() {
        found.notes.push(copy::NOTE_HELM_NO_CLUSTER.to_string());
        found.incomplete = true;
        return found;
    }
    let releases: Vec<HelmRelease> = serde_json::from_str(&out.stdout).unwrap_or_default();
    if releases.is_empty() {
        return found;
    }
    let repos: Vec<Repo> = serde_json::from_str(
        &exec::run(
            "helm",
            &["repo", "list", "--output", "json"],
            Duration::from_secs(10),
        )
        .stdout,
    )
    .unwrap_or_default();
    // Reads each repo's index over HTTP into update-all's own cache, which
    // leaves helm's repo cache exactly as it was.
    let indexes: Vec<(Repo, Option<Index>)> = par_map(repos, 4, |repo| {
        let url = format!("{}/index.yaml", repo.url.trim_end_matches('/'));
        let index = http::get_cached(&url, Duration::from_secs(3600))
            .ok()
            .and_then(|t| serde_yaml_ng::from_str::<Index>(&t).ok());
        (repo, index)
    });
    let cache_dir = exec::run(
        "helm",
        &["env", "HELM_REPOSITORY_CACHE"],
        Duration::from_secs(5),
    )
    .stdout
    .trim()
    .to_string();

    let mut report = Report {
        title: copy::REPORT_HELM_TITLE.to_string(),
        lines: Vec::new(),
    };
    let mut unknown = Vec::new();
    for rel in &releases {
        let Some((repo, chart, current, versions)) = find_chart(&rel.chart, &indexes) else {
            unknown.push(rel.chart.clone());
            continue;
        };
        let Some(latest) = versions
            .iter()
            .filter(|v| !v.deprecated && !version::clean(&v.version).contains('-'))
            .max_by(|a, b| version::compare(&a.version, &b.version))
        else {
            continue;
        };
        if !version::is_newer(&latest.version, &current) {
            continue;
        }
        let date_of = |v: &str| {
            versions
                .iter()
                .find(|c| c.version == v)
                .and_then(|c| c.created.as_deref())
                .and_then(dates::parse)
        };
        let cur = Release::new(&current, date_of(&current));
        let new = Release::new(&latest.version, date_of(&latest.version));
        let age = Age::between(&cur, &new, jiff::Timestamp::now());
        report.lines.push(fill(
            copy::REPORT_HELM_LINE,
            &[
                &rel.name,
                &rel.namespace,
                &format!("{}/{chart}", repo.name),
                &cur.version,
                &date_label(cur.date),
                &new.version,
                &date_label(new.date),
                &age.label(),
            ],
        ));
        if repo_cache_is_stale(&cache_dir, &repo.name, new.date) {
            report
                .lines
                .push(format!("    helm repo update {}", repo.name));
        }
        report.lines.push(format!(
            "    helm upgrade {} {}/{chart} --namespace {} --version {} --reuse-values",
            rel.name, repo.name, rel.namespace, latest.version
        ));
    }
    if !unknown.is_empty() {
        found
            .notes
            .push(fill(copy::NOTE_HELM_UNKNOWN_CHARTS, &[&unknown.join(", ")]));
    }
    if !report.lines.is_empty() {
        report.lines.insert(0, copy::REPORT_HELM_WHY.to_string());
        found.reports.push(report);
    }
    found
}

/// Splits `kube-prometheus-stack-65.1.0` into chart and version by finding a
/// repo that has the chart. Names can contain dashes, so this tries each split.
fn find_chart<'a>(
    chart_field: &str,
    indexes: &'a [(Repo, Option<Index>)],
) -> Option<(&'a Repo, String, String, &'a [ChartVersion])> {
    for (pos, _) in chart_field.match_indices('-') {
        let (name, ver) = (&chart_field[..pos], &chart_field[pos + 1..]);
        if !ver
            .trim_start_matches('v')
            .starts_with(|c: char| c.is_ascii_digit())
        {
            continue;
        }
        for (repo, index) in indexes {
            if let Some(versions) = index.as_ref().and_then(|i| i.entries.get(name)) {
                return Some((repo, name.to_string(), ver.to_string(), versions));
            }
        }
    }
    None
}

/// True when helm's local copy of a repo index predates the newest chart, so
/// `helm upgrade --version` would fail until the user runs `helm repo update`.
fn repo_cache_is_stale(cache_dir: &str, repo: &str, latest: Option<jiff::Timestamp>) -> bool {
    let path = Path::new(cache_dir).join(format!("{repo}-index.yaml"));
    let Ok(modified) = fs::metadata(path).and_then(|m| m.modified()) else {
        return true;
    };
    let secs = modified
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    latest.is_some_and(|l| l.as_second() > secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_chart_names_with_dashes() {
        let index: Index = serde_yaml_ng::from_str(
            "entries:\n  kube-prometheus-stack:\n    - version: 66.0.0\n      created: 2026-10-01T00:00:00Z\n",
        )
        .unwrap();
        let indexes = vec![(
            Repo {
                name: "prometheus-community".into(),
                url: "https://example.com".into(),
            },
            Some(index),
        )];
        let (repo, chart, ver, versions) =
            find_chart("kube-prometheus-stack-65.1.0", &indexes).unwrap();
        assert_eq!(repo.name, "prometheus-community");
        assert_eq!(chart, "kube-prometheus-stack");
        assert_eq!(ver, "65.1.0");
        assert_eq!(versions[0].version, "66.0.0");
        assert!(find_chart("cert-manager-v1.15.0", &indexes).is_none());
    }
}
