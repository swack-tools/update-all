//! Runs the selected updates in dependency order and records what happened.

use std::collections::{HashMap, HashSet};

use crate::copy::{self, fill};
use crate::exec;
use crate::model::{Action, Item, Source};
use crate::sources::{appstore, brew, docker, sparkle};
use crate::ui;

#[derive(Debug)]
pub enum Status {
    Updated,
    Failed(String),
    /// Waiting for the user, for example to quit a running app.
    Queued(String),
    /// update-all started the app's own updater, and the update finishes when
    /// the app relaunches.
    Requested,
}

#[derive(Debug)]
pub struct Outcome {
    pub name: String,
    pub status: Status,
}

fn announce(item: &Item) {
    println!(
        "\n{} {}  {} -> {}",
        ui::bold("==>"),
        ui::bold(&item.name),
        item.current.version,
        item.latest.version
    );
}

fn needs_sudo(item: &Item) -> bool {
    matches!(
        item.action,
        Action::Command { sudo: true, .. } | Action::SoftwareUpdate { .. }
    )
}

/// Applies every item that doesn't need a restart.
pub fn run(items: &[&Item]) -> Vec<Outcome> {
    let mut items: Vec<&Item> = items.to_vec();
    items.sort_by_key(|i| i.source);
    let mut outcomes = Vec::new();

    let sudo_ok = !items.iter().any(|i| needs_sudo(i)) || {
        println!("{}", copy::ASK_SUDO);
        exec::sudo_start()
    };

    // Homebrew goes first in one batch per kind, since other tools depend on it.
    for (kind, pick) in [("--formula", true), ("--cask", false)] {
        let batch: Vec<&Item> = items
            .iter()
            .copied()
            .filter(|i| match &i.action {
                Action::BrewFormula(_) => pick,
                Action::BrewCask(_) => !pick,
                _ => false,
            })
            .collect();
        if batch.is_empty() {
            continue;
        }
        let names: Vec<String> = batch
            .iter()
            .map(|i| match &i.action {
                Action::BrewFormula(n) | Action::BrewCask(n) => n.clone(),
                _ => unreachable!(),
            })
            .collect();
        println!(
            "\n{} {}",
            ui::bold("==>"),
            ui::bold(&exec::display_owned(
                "brew",
                &brew::upgrade_args(kind, &names)
            ))
        );
        let failed = brew::upgrade(kind, &names);
        for (item, name) in batch.iter().zip(&names) {
            outcomes.push(Outcome {
                name: item.name.clone(),
                status: if failed.contains(name) {
                    Status::Failed(copy::ERR_STILL_OUTDATED.to_string())
                } else {
                    Status::Updated
                },
            });
        }
    }

    let mut ran: HashMap<String, bool> = HashMap::new();
    let mut limited: HashSet<String> = HashSet::new();
    for item in items {
        let status = match &item.action {
            Action::BrewFormula(_) | Action::BrewCask(_) => continue,
            Action::Command {
                program,
                args,
                sudo,
            } => {
                let key = exec::display_owned(program, args);
                if let Some(ok) = ran.get(&key) {
                    // Several items can share one command, like `gcloud components update`.
                    if *ok {
                        Status::Updated
                    } else {
                        Status::Failed(copy::ERR_COMMAND_FAILED.to_string())
                    }
                } else if *sudo && !sudo_ok {
                    Status::Failed(copy::ERR_NO_SUDO.to_string())
                } else {
                    announce(item);
                    println!("{}", ui::dim(&key));
                    let ok = exec::run_live(program, args, *sudo);
                    ran.insert(key, ok);
                    if ok {
                        Status::Updated
                    } else {
                        Status::Failed(copy::ERR_COMMAND_FAILED.to_string())
                    }
                }
            }
            Action::Sparkle(update) => {
                announce(item);
                println!(
                    "{}",
                    ui::dim(&fill(copy::SPARKLE_DOWNLOADING, &[&update.item.url]))
                );
                match sparkle::install(update) {
                    Ok(()) => {
                        println!("{}", fill(copy::SPARKLE_INSTALLED, &[&update.item.display]));
                        Status::Updated
                    }
                    Err(e) if e.to_string() == copy::ERR_APP_RUNNING => {
                        Status::Queued(copy::QUEUED_QUIT_APP.to_string())
                    }
                    Err(e) => Status::Failed(e.to_string()),
                }
            }
            Action::DockerPull(reference) => {
                let registry = docker::registry(reference).to_string();
                if limited.contains(&registry) {
                    Status::Failed(copy::ERR_DOCKER_RATE_LIMITED.to_string())
                } else {
                    announce(item);
                    match docker::pull(reference) {
                        docker::Pull::Done => Status::Updated,
                        docker::Pull::RateLimited => {
                            // More pulls from this registry would only count against its limit.
                            limited.insert(registry);
                            Status::Failed(copy::ERR_DOCKER_RATE_LIMITED.to_string())
                        }
                        docker::Pull::Failed => {
                            Status::Failed(copy::ERR_COMMAND_FAILED.to_string())
                        }
                    }
                }
            }
            Action::LaunchHidden { app } => {
                announce(item);
                if exec::run_live(
                    "open",
                    &["-g".into(), "-j".into(), "-a".into(), app.clone()],
                    false,
                ) {
                    Status::Requested
                } else {
                    Status::Failed(copy::ERR_COMMAND_FAILED.to_string())
                }
            }
            Action::AppStore { adam_id, app } => {
                announce(item);
                match appstore::update(*adam_id, app) {
                    Ok(()) => Status::Updated,
                    Err(e) => Status::Failed(e.to_string()),
                }
            }
            Action::SoftwareUpdate { label } => {
                if !sudo_ok {
                    Status::Failed(copy::ERR_NO_SUDO.to_string())
                } else {
                    announce(item);
                    let args = vec!["--install".to_string(), label.clone()];
                    if exec::run_live("softwareupdate", &args, true) {
                        Status::Updated
                    } else {
                        Status::Failed(copy::ERR_COMMAND_FAILED.to_string())
                    }
                }
            }
        };
        outcomes.push(Outcome {
            name: item.name.clone(),
            status,
        });
    }
    exec::sudo_stop();
    outcomes
}

/// Installs the updates that need a restart, then restarts the Mac. Call it
/// only after the user confirms, and after the final report prints.
pub fn run_restart(items: &[&Item]) {
    let casks: Vec<String> = items
        .iter()
        .filter_map(|i| match &i.action {
            Action::BrewCask(n) => Some(n.clone()),
            _ => None,
        })
        .collect();
    let labels: Vec<String> = items
        .iter()
        .filter_map(|i| match &i.action {
            Action::SoftwareUpdate { label } => Some(label.clone()),
            _ => None,
        })
        .collect();
    if !exec::sudo_start() {
        println!("{}", copy::ERR_NO_SUDO);
        return;
    }
    if !casks.is_empty() {
        brew::upgrade("--cask", &casks);
    }
    for item in items
        .iter()
        .filter(|i| i.source != Source::MacOs && !matches!(i.action, Action::BrewCask(_)))
    {
        if let Action::Command {
            program,
            args,
            sudo,
        } = &item.action
        {
            exec::run_live(program, args, *sudo);
        }
    }
    if labels.is_empty() {
        exec::sudo_stop();
        println!("{}", copy::RESTARTING);
        exec::run_live(
            "osascript",
            &[
                "-e".into(),
                "tell application \"System Events\" to restart".into(),
            ],
            false,
        );
        return;
    }
    let mut args = vec!["--install".to_string()];
    args.extend(labels);
    // softwareupdate restarts the Mac itself once the install finishes.
    args.push("--restart".to_string());
    println!(
        "{}",
        fill(
            copy::RESTARTING_AFTER,
            &[&exec::display_owned("softwareupdate", &args)]
        )
    );
    exec::run_live("softwareupdate", &args, true);
    exec::sudo_stop();
}
