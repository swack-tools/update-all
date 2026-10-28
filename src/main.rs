//! update-all: one command that checks everything on a Mac for updates,
//! shows how far behind each item is, and updates what you choose.

mod apply;
mod config;
mod copy;
mod dates;
mod exec;
mod http;
mod model;
mod origin;
mod sources;
mod state;
mod tui;
mod ui;
mod version;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use clap::{ArgAction, Parser};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

use crate::apply::Status;
use crate::config::Config;
use crate::copy::fill;
use crate::model::{Action, Discovery, Item, Report};
use crate::sources::{CHECKS, Ctx};
use crate::state::{PendingRestart, State};

#[derive(Parser)]
#[command(
    name = "update-all",
    version,
    about = copy::ABOUT,
    after_help = copy::SHORT_FOOTER,
    after_long_help = copy::LONG_HELP,
    disable_help_flag = true,
    disable_version_flag = true
)]
struct Cli {
    #[arg(short = 'n', long, help = copy::ARG_DRY_RUN)]
    dry_run: bool,
    #[arg(short = 'y', long, help = copy::ARG_YES)]
    yes: bool,
    #[arg(long, help = copy::ARG_LITERALLY_ALL)]
    update_literally_all: bool,
    #[arg(long, help = copy::ARG_ELECTRON)]
    electron: bool,
    #[arg(long, help = copy::ARG_ELECTRON_ONLY)]
    electron_only: bool,
    #[arg(long, help = copy::ARG_APP_STORE)]
    app_store: bool,
    #[arg(long, help = copy::ARG_APP_STORE_ONLY)]
    app_store_only: bool,
    #[arg(long, help = copy::ARG_CONFIGURE_SHELLS)]
    configure_shells: bool,
    #[arg(long, value_name = "PATH", help = copy::ARG_CONFIG)]
    config: Option<PathBuf>,
    #[arg(short = 'h', long, action = ArgAction::Help, help = copy::ARG_HELP)]
    help: Option<bool>,
    #[arg(short = 'V', long, action = ArgAction::Version, help = copy::ARG_VERSION)]
    version: Option<bool>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if !cfg!(target_os = "macos") {
        eprintln!("{}", copy::ERR_NOT_MACOS);
        return ExitCode::from(2);
    }
    let log = exec::open_log();
    let config = match Config::load(cli.config.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}", fill(copy::ERR_CONFIG, &[&format!("{e:#}")]));
            return ExitCode::from(2);
        }
    };
    let mut state = State::load();
    if let Some(pending) = state.pending_restart.take() {
        println!(
            "{}",
            fill(
                copy::CONTINUING_AFTER_RESTART,
                &[&pending.updates.join(", ")]
            )
        );
        state.save();
    }

    shell_screen(&cli, &mut state);

    let only = cli.electron_only || cli.app_store_only;
    let found = discover(&cli, &config, &state);
    let mut items = config.apply(found.items);
    if only {
        items
            .retain(|i| (cli.electron_only && i.electron) || (cli.app_store_only && i.app_store()));
    }

    // Held items show up in the summary but never update.
    let (held, items): (Vec<Item>, Vec<Item>) = items.into_iter().partition(|i| i.held);
    let electron_n = items.iter().filter(|i| i.electron).count();
    let store_n = items.iter().filter(|i| i.app_store()).count();

    print_summary(&items, &held, &found.reports, &found.notes);

    if items.is_empty() {
        println!(
            "\n{}",
            if found.incomplete {
                copy::ALL_CURRENT_PARTIAL
            } else {
                copy::ALL_CURRENT
            }
        );
        return ExitCode::SUCCESS;
    }

    let mut include_electron = cli.electron || cli.electron_only;
    let mut include_store = cli.app_store || cli.app_store_only;

    if cli.dry_run {
        print_dry_run(&items, include_electron, include_store);
        if let Some(path) = &log {
            println!(
                "\n{}",
                ui::dim(&fill(copy::LOG_AT, &[&path.display().to_string()]))
            );
        }
        return ExitCode::SUCCESS;
    }

    let can_ask = ui::interactive() && !cli.yes;
    if !cli.yes && !ui::interactive() {
        println!("\n{}", copy::NOT_INTERACTIVE);
        return ExitCode::SUCCESS;
    }
    if can_ask && !only {
        if electron_n > 0 && !include_electron {
            include_electron =
                ui::confirm(&fill(copy::ASK_ELECTRON, &[&electron_n.to_string()]), false);
        }
        if store_n > 0 && !include_store {
            include_store = ui::confirm(&fill(copy::ASK_APP_STORE, &[&store_n.to_string()]), false);
        }
    }
    let in_scope =
        |i: &Item| (!i.electron || include_electron) && (!i.app_store() || include_store);
    let mut checked: Vec<bool> = items.iter().map(in_scope).collect();

    if can_ask {
        let count = checked.iter().filter(|c| **c).count();
        loop {
            match ui::ask(&fill(copy::ASK_MAIN, &[&count.to_string()])).as_str() {
                "y" | "yes" => break,
                "n" | "no" | "q" => {
                    println!("{}", copy::NOTHING_CHANGED);
                    return ExitCode::SUCCESS;
                }
                "p" | "pick" => match tui::pick(&items, checked.clone()) {
                    Some(c) => {
                        checked = c;
                        break;
                    }
                    None => {
                        println!("{}", copy::NOTHING_CHANGED);
                        return ExitCode::SUCCESS;
                    }
                },
                _ => println!("{}", copy::ASK_MAIN_HELP),
            }
        }
    }

    let chosen: Vec<&Item> = items
        .iter()
        .zip(&checked)
        .filter(|(_, c)| **c)
        .map(|(i, _)| i)
        .collect();
    if chosen.is_empty() {
        println!("{}", copy::NOTHING_CHANGED);
        return ExitCode::SUCCESS;
    }
    let (later, now): (Vec<&Item>, Vec<&Item>) = chosen.into_iter().partition(|i| i.needs_restart);
    let outcomes = apply::run(&now);
    let failed = print_outcomes(&outcomes);

    if !later.is_empty() {
        restart_step(&later, &mut state);
    }
    if let Some(path) = &log {
        println!(
            "\n{}",
            ui::dim(&fill(copy::LOG_AT, &[&path.display().to_string()]))
        );
    }
    if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// Shows the shell screen on the first run, when a new framework appears,
/// or when the user asks for it with --configure-shells.
fn shell_screen(cli: &Cli, state: &mut State) {
    let found = sources::shells::detect_all();
    let unseen = found
        .iter()
        .any(|f| !state.shells_seen.iter().any(|s| s == f.id));
    if cli.configure_shells && found.is_empty() {
        println!("{}", copy::SHELL_NONE_FOUND);
        return;
    }
    if found.is_empty() || !(unseen || cli.configure_shells) || !ui::interactive() || cli.yes {
        return;
    }
    if let Some(enabled) = tui::shell_screen(&found, &state.shells_enabled) {
        state.shells_enabled = enabled;
        state.shells_seen = found.iter().map(|f| f.id.to_string()).collect();
        state.save();
    }
}

fn discover(cli: &Cli, config: &Config, state: &State) -> Discovery {
    let ctx = Ctx {
        config,
        literally_all: cli.update_literally_all,
        shells_enabled: &state.shells_enabled,
    };
    let checks: Vec<_> = CHECKS
        .iter()
        .filter(|c| {
            if cli.electron_only || cli.app_store_only {
                (cli.electron_only
                    && (c.name == copy::LABEL_BREW || c.name == copy::LABEL_APPS_AND_BROWSERS))
                    || (cli.app_store_only && c.name == copy::LABEL_APP_STORE)
            } else {
                true
            }
        })
        .collect();
    println!("{}", ui::bold(copy::CHECKING));
    let bars = MultiProgress::new();
    if !ui::interactive() {
        bars.set_draw_target(indicatif::ProgressDrawTarget::hidden());
    }
    let style = ProgressStyle::with_template("  {spinner} {prefix:<24} {msg}")
        .unwrap_or_else(|_| ProgressStyle::default_spinner());
    let all = Mutex::new(Discovery::default());
    thread::scope(|scope| {
        for check in checks {
            let bar = bars.add(ProgressBar::new_spinner());
            bar.set_style(style.clone());
            bar.set_prefix(check.name);
            bar.set_message(copy::SPINNER_CHECKING);
            bar.enable_steady_tick(Duration::from_millis(120));
            let (ctx, all) = (&ctx, &all);
            scope.spawn(move || {
                let found = (check.run)(ctx);
                let n = found
                    .items
                    .iter()
                    .filter(|i| !ctx.config.skipped(i))
                    .count();
                bar.finish_with_message(match n {
                    0 => copy::SPINNER_NONE.to_string(),
                    1 => copy::SPINNER_ONE.to_string(),
                    n => fill(copy::SPINNER_MANY, &[&n.to_string()]),
                });
                if let Ok(mut a) = all.lock() {
                    a.items.extend(found.items);
                    a.reports.extend(found.reports);
                    a.notes.extend(found.notes);
                    a.incomplete |= found.incomplete;
                }
            });
        }
    });
    all.into_inner().unwrap_or_default()
}

fn print_summary(items: &[Item], held: &[Item], reports: &[Report], notes: &[String]) {
    let mut sources: Vec<_> = items.iter().map(|i| i.source).collect();
    sources.sort();
    sources.dedup();
    for source in sources {
        let group: Vec<&Item> = items.iter().filter(|i| i.source == source).collect();
        ui::heading(&format!("{} ({})", source.label(), group.len()));
        ui::print_items(&group);
    }
    if !held.is_empty() {
        ui::heading(copy::HEADING_HELD);
        ui::print_items(&held.iter().collect::<Vec<_>>());
    }
    for report in reports {
        ui::print_report(report);
    }
    if !notes.is_empty() {
        ui::heading(copy::HEADING_NOTES);
        for note in notes {
            println!("  - {note}");
        }
    }
}

fn print_dry_run(items: &[Item], electron: bool, store: bool) {
    ui::heading(copy::HEADING_DRY_RUN);
    let mut sorted: Vec<&Item> = items.iter().collect();
    sorted.sort_by_key(|i| (i.needs_restart, i.source));
    for item in sorted {
        let optin = (item.electron && !electron) || (item.app_store() && !store);
        let what = describe(&item.action);
        let mut line = format!("  {}  {}", item.name, ui::dim(&what));
        if optin {
            line.push_str(&format!("  {}", ui::dim(copy::TAG_OPT_IN)));
        }
        if item.needs_restart {
            line.push_str(&format!("  {}", ui::dim(copy::TAG_RESTART_LAST)));
        }
        println!("{line}");
    }
    println!("\n{}", copy::DRY_RUN_DONE);
}

fn describe(action: &Action) -> String {
    match action {
        Action::Command {
            program,
            args,
            sudo,
        } => {
            let cmd = exec::display_owned(program, args);
            if *sudo { format!("sudo {cmd}") } else { cmd }
        }
        Action::BrewFormula(n) => format!("brew upgrade --formula {n}"),
        Action::BrewCask(n) => format!("brew upgrade --cask --greedy {n}"),
        Action::Sparkle(u) => fill(copy::DESCRIBE_SPARKLE, &[&u.item.url]),
        Action::DockerPull(reference) => exec::display("docker", &["pull", reference]),
        Action::LaunchHidden { app } => exec::display("open", &["-g", "-j", "-a", app]),
        Action::AppStore { adam_id, .. } => fill(copy::DESCRIBE_APP_STORE, &[&adam_id.to_string()]),
        Action::SoftwareUpdate { label } => {
            exec::display("sudo softwareupdate --install", &[label])
        }
    }
}

/// Prints the final report. Returns true if anything failed.
fn print_outcomes(outcomes: &[apply::Outcome]) -> bool {
    ui::heading(copy::HEADING_RESULTS);
    let mut failed = false;
    for o in outcomes {
        let (mark, detail) = match &o.status {
            Status::Updated => (
                ui::paint(
                    copy::STATUS_UPDATED,
                    Some(ratatui::crossterm::style::Color::Green),
                ),
                String::new(),
            ),
            Status::Requested => (
                ui::paint(
                    copy::STATUS_REQUESTED,
                    Some(ratatui::crossterm::style::Color::Cyan),
                ),
                copy::REQUESTED.to_string(),
            ),
            Status::Queued(why) => (
                ui::paint(
                    copy::STATUS_QUEUED,
                    Some(ratatui::crossterm::style::Color::Yellow),
                ),
                why.clone(),
            ),
            Status::Failed(why) => {
                failed = true;
                (
                    ui::paint(
                        copy::STATUS_FAILED,
                        Some(ratatui::crossterm::style::Color::Red),
                    ),
                    why.clone(),
                )
            }
        };
        println!("  {mark:>9}  {}  {}", o.name, ui::dim(&detail));
    }
    failed
}

fn restart_step(later: &[&Item], state: &mut State) {
    ui::heading(copy::HEADING_RESTART);
    for item in later {
        println!(
            "  {}  {} -> {}",
            item.name, item.current.version, item.latest.version
        );
    }
    println!("\n{}", copy::RESTART_RERUN);
    // This question is always asked, even with --yes, because a restart can
    // close unsaved work.
    if !ui::interactive() || !ui::confirm(copy::ASK_RESTART, false) {
        println!("{}", copy::RESTART_SKIPPED);
        return;
    }
    state.pending_restart = Some(PendingRestart {
        at: jiff::Timestamp::now().to_string(),
        updates: later.iter().map(|i| i.name.clone()).collect(),
    });
    state.save();
    apply::run_restart(later);
}
