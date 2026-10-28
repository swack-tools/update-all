//! AI command-line tools that install and update themselves. Tools that came
//! from Homebrew, npm, uv, or pipx update with those package managers instead.

use std::fs;
use std::time::Duration;

use serde::Deserialize;

use super::{Ctx, par_map};
use crate::dates;
use crate::exec;
use crate::http;
use crate::model::{Action, Discovery, Item, Release, Source};
use crate::origin::{self, Origin};
use crate::version;

enum Latest {
    /// Claude Code's release bucket, which follows the user's update channel.
    ClaudeCode,
    /// A URL that returns the version as plain text.
    Text(&'static str),
    /// A URL that returns JSON with the version in a top-level field.
    Json(&'static str, &'static str),
    /// The newest GitHub release, minus a tag prefix such as `v`.
    GitHub(&'static str, &'static str, &'static str),
}

struct Tool {
    bin: &'static str,
    name: &'static str,
    description: &'static str,
    latest: Latest,
    update: &'static [&'static str],
}

const TOOLS: &[Tool] = &[
    Tool {
        bin: "claude",
        name: "Claude Code",
        description: "Anthropic's coding agent for the terminal",
        latest: Latest::ClaudeCode,
        update: &["update"],
    },
    Tool {
        bin: "agy",
        name: "Antigravity CLI",
        description: "Google Antigravity's coding agent for the terminal",
        latest: Latest::Json(
            "https://antigravity-cli-auto-updater-974169037036.us-central1.run.app/manifests/darwin_arm64.json",
            "version",
        ),
        update: &["update"],
    },
    Tool {
        bin: "opencode",
        name: "opencode",
        description: "Open source coding agent for the terminal",
        latest: Latest::GitHub("sst", "opencode", "v"),
        update: &["upgrade"],
    },
    Tool {
        bin: "copilot",
        name: "GitHub Copilot CLI",
        description: "GitHub Copilot's coding agent for the terminal",
        latest: Latest::GitHub("github", "copilot-cli", "v"),
        update: &["update"],
    },
    Tool {
        bin: "goose",
        name: "goose",
        description: "Open source AI agent for the terminal",
        latest: Latest::GitHub("aaif-goose", "goose", "v"),
        update: &["update"],
    },
    Tool {
        bin: "kiro-cli",
        name: "Kiro CLI",
        description: "Kiro's coding agent for the terminal",
        latest: Latest::Json(
            "https://desktop-release.q.us-east-1.amazonaws.com/latest/manifest.json",
            "version",
        ),
        update: &["update", "--non-interactive"],
    },
    Tool {
        bin: "amp",
        name: "Amp",
        description: "Sourcegraph's coding agent for the terminal",
        latest: Latest::Text("https://static.ampcode.com/cli/cli-version.txt"),
        update: &["update"],
    },
    Tool {
        bin: "droid",
        name: "Droid",
        description: "Factory's coding agent for the terminal",
        latest: Latest::Text("https://downloads.factory.ai/factory-cli/LATEST"),
        update: &["update"],
    },
];

/// Registry descriptions, for the Vale check.
#[cfg(test)]
pub fn described() -> Vec<(String, &'static str)> {
    TOOLS
        .iter()
        .map(|t| (format!("ai {}", t.bin), t.description))
        .collect()
}

pub fn discover(_ctx: &Ctx) -> Discovery {
    let mut found = Discovery::default();
    let tools: Vec<&Tool> = TOOLS
        .iter()
        .filter(|t| exec::which(t.bin).is_some_and(|p| origin::of(&p) == Origin::Native))
        .collect();
    found.items = par_map(tools, 6, check).into_iter().flatten().collect();
    found
}

#[derive(Deserialize)]
struct GhLatest {
    tag_name: String,
    published_at: Option<String>,
}

#[derive(Deserialize)]
struct ClaudeManifest {
    #[serde(rename = "buildDate")]
    build_date: Option<String>,
}

const CLAUDE_RELEASES: &str = "https://downloads.claude.ai/claude-code-releases";

fn check(tool: &Tool) -> Option<Item> {
    let path = exec::which(tool.bin)?;
    let current = current_version(tool)?;
    let (latest, cur_date, new_date) = match tool.latest {
        Latest::ClaudeCode => {
            let latest = http::get_text(&format!("{CLAUDE_RELEASES}/{}", claude_channel()))
                .ok()?
                .trim()
                .to_string();
            let built = |v: &str| {
                http::get_json::<ClaudeManifest>(&format!("{CLAUDE_RELEASES}/{v}/manifest.json"))
                    .ok()
                    .and_then(|m| m.build_date)
                    .and_then(|d| dates::parse(&d))
            };
            if !version::is_newer(&latest, &current) {
                return None;
            }
            let (c, n) = (built(&current), built(&latest));
            (latest, c, n)
        }
        Latest::Text(url) => (http::get_text(url).ok()?.trim().to_string(), None, None),
        Latest::Json(url, field) => {
            let v: serde_json::Value = http::get_json(url).ok()?;
            (v.get(field)?.as_str()?.to_string(), None, None)
        }
        Latest::GitHub(owner, repo, prefix) => {
            let rel: GhLatest =
                http::github(&format!("repos/{owner}/{repo}/releases/latest")).ok()?;
            let latest = rel.tag_name.trim_start_matches(prefix).to_string();
            let cur_date = version::is_newer(&latest, &current)
                .then(|| dates::github_tag_date(owner, repo, &format!("{prefix}{current}")))
                .flatten();
            (
                latest,
                cur_date,
                rel.published_at.as_deref().and_then(dates::parse),
            )
        }
    };
    if !version::is_newer(&latest, &current) {
        return None;
    }
    Some(
        Item::new(
            Source::Ai,
            format!("ai:{}", tool.bin),
            tool.name,
            Action::Command {
                program: path.display().to_string(),
                args: tool.update.iter().map(|a| a.to_string()).collect(),
                sudo: false,
            },
        )
        .path(path.display().to_string())
        .description(tool.description)
        .versions(
            Release::new(current, cur_date),
            Release::new(latest, new_date),
        ),
    )
}

fn current_version(tool: &Tool) -> Option<String> {
    let path = exec::which(tool.bin)?;
    // Claude Code links ~/.local/bin/claude to versions/<version>.
    if tool.bin == "claude"
        && let Ok(real) = fs::canonicalize(&path)
        && let Some(name) = real.file_name().and_then(|n| n.to_str())
        && name.chars().next().is_some_and(|c| c.is_ascii_digit())
    {
        return Some(name.to_string());
    }
    let out = exec::run(tool.bin, &["--version"], Duration::from_secs(15));
    version::find_in(&out.stdout).or_else(|| version::find_in(&out.stderr))
}

/// Claude Code's `autoUpdatesChannel` setting. It defaults to the newest
/// builds, and `stable` trails it.
fn claude_channel() -> String {
    fs::read_to_string(exec::home().join(".claude/settings.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("autoUpdatesChannel")?.as_str().map(str::to_string))
        .filter(|c| c == "stable" || c == "latest")
        .unwrap_or_else(|| "latest".to_string())
}
