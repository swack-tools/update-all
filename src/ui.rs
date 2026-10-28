//! Plain terminal output: the summary, colors, and yes or no prompts.

use std::io::{self, BufRead, IsTerminal, Write};

use ratatui::crossterm::style::{Color, Stylize};

use crate::model::{AgeLevel, Item, Report, date_label};

pub fn color_on() -> bool {
    std::env::var_os("NO_COLOR").is_none() && io::stdout().is_terminal()
}

pub fn interactive() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

fn age_color(level: AgeLevel) -> Option<Color> {
    match level {
        AgeLevel::Plain => None,
        AgeLevel::Yellow => Some(Color::Yellow),
        AgeLevel::Orange => Some(Color::AnsiValue(208)),
        AgeLevel::Red => Some(Color::Red),
    }
}

pub fn bold(text: &str) -> String {
    if color_on() {
        text.bold().to_string()
    } else {
        text.to_string()
    }
}

pub fn dim(text: &str) -> String {
    if color_on() {
        text.dim().to_string()
    } else {
        text.to_string()
    }
}

pub fn paint(text: &str, color: Option<Color>) -> String {
    match color {
        Some(c) if color_on() => text.with(c).to_string(),
        _ => text.to_string(),
    }
}

pub fn heading(text: &str) {
    println!("\n{}", bold(text));
}

fn truncate(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width {
        format!("{text:width$}")
    } else {
        let cut: String = text.chars().take(width.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

/// Prints items as aligned rows: name, current version and date, new
/// version and date, and the colored age.
pub fn print_items(items: &[&Item]) {
    let name_w = items
        .iter()
        .map(|i| i.name.chars().count())
        .max()
        .unwrap_or(0)
        .min(36);
    let cur_w = items
        .iter()
        .map(|i| i.current.version.chars().count())
        .max()
        .unwrap_or(0)
        .min(20);
    let new_w = items
        .iter()
        .map(|i| i.latest.version.chars().count())
        .max()
        .unwrap_or(0)
        .min(20);
    for it in items {
        let age = it.age();
        let mut line = format!(
            "  {}  {} {}  ->  {} {}  {}",
            truncate(&it.name, name_w),
            truncate(&it.current.version, cur_w),
            dim(&format!("{:10}", date_label(it.current.date))),
            truncate(&it.latest.version, new_w),
            dim(&format!("{:10}", date_label(it.latest.date))),
            paint(&format!("{:>5}", age.label()), age_color(age.level())),
        );
        let mut tags = Vec::new();
        if it.electron {
            tags.push(crate::copy::TAG_ELECTRON.to_string());
        }
        if it.needs_restart {
            tags.push(crate::copy::TAG_RESTART.to_string());
        }
        if let Some(n) = &it.note {
            tags.push(n.clone());
        }
        if !tags.is_empty() {
            line.push_str(&format!("  {}", dim(&tags.join("; "))));
        }
        println!("{line}");
    }
}

pub fn print_report(report: &Report) {
    heading(&report.title);
    for line in &report.lines {
        println!("  {line}");
    }
}

/// Reads one line from the terminal after printing `prompt`.
pub fn ask(prompt: &str) -> String {
    print!("{prompt} ");
    let _ = io::stdout().flush();
    let mut line = String::new();
    let _ = io::stdin().lock().read_line(&mut line);
    line.trim().to_lowercase()
}

/// A yes or no question. An empty answer picks `default`.
pub fn confirm(prompt: &str, default: bool) -> bool {
    let hint = if default { "[Y/n]" } else { "[y/N]" };
    match ask(&format!("{prompt} {hint}")).as_str() {
        "" => default,
        a => a.starts_with('y'),
    }
}
