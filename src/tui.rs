//! Full-screen pickers: the update picker and the first-run shell screen.

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, TableState, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::copy::{self, fill};
use crate::model::{AgeLevel, Item, date_label};
use crate::sources::shells::Found;

pub fn age_color(level: AgeLevel) -> Option<Color> {
    match level {
        AgeLevel::Plain => None,
        AgeLevel::Yellow => Some(Color::Yellow),
        AgeLevel::Orange => Some(Color::Indexed(208)),
        AgeLevel::Red => Some(Color::Red),
    }
}

struct Picker<'a> {
    items: &'a [Item],
    checked: Vec<bool>,
    /// Indexes into `items` that match the filter, in display order.
    visible: Vec<usize>,
    order: Vec<usize>,
    state: TableState,
    filter: String,
    typing: bool,
}

impl Picker<'_> {
    fn refilter(&mut self) {
        let f = self.filter.to_lowercase();
        self.visible = self
            .order
            .iter()
            .copied()
            .filter(|&i| {
                let it = &self.items[i];
                f.is_empty()
                    || it.name.to_lowercase().contains(&f)
                    || it.id.to_lowercase().contains(&f)
                    || it.path.to_lowercase().contains(&f)
                    || it.description.to_lowercase().contains(&f)
            })
            .collect();
        let sel = self.state.selected().unwrap_or(0);
        self.state.select(if self.visible.is_empty() {
            None
        } else {
            Some(sel.min(self.visible.len() - 1))
        });
    }

    fn current(&self) -> Option<usize> {
        self.state
            .selected()
            .and_then(|s| self.visible.get(s).copied())
    }

    fn move_by(&mut self, delta: isize) {
        if self.visible.is_empty() {
            return;
        }
        let max = self.visible.len() as isize - 1;
        let cur = self.state.selected().unwrap_or(0) as isize;
        self.state
            .select(Some((cur + delta).clamp(0, max) as usize));
    }
}

/// Shows every update with a checkbox. Returns the new checked list, or None
/// when the user quits without confirming.
pub fn pick(items: &[Item], checked: Vec<bool>) -> Option<Vec<bool>> {
    let mut order: Vec<usize> = (0..items.len()).collect();
    // Most out-of-date first; unknown ages go last.
    order.sort_by_key(|&i| std::cmp::Reverse(items[i].age().days().unwrap_or(-1)));
    let mut picker = Picker {
        items,
        checked,
        visible: order.clone(),
        order,
        state: TableState::default().with_selected(Some(0)),
        filter: String::new(),
        typing: false,
    };
    let mut terminal = ratatui::init();
    let result = run_picker(&mut terminal, &mut picker);
    ratatui::restore();
    result
}

fn run_picker(terminal: &mut DefaultTerminal, p: &mut Picker) -> Option<Vec<bool>> {
    loop {
        if terminal.draw(|f| draw_picker(f, p)).is_err() {
            return None;
        }
        let Ok(Event::Key(key)) = event::read() else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return None;
        }
        if p.typing {
            match key.code {
                KeyCode::Enter | KeyCode::Esc => p.typing = false,
                KeyCode::Backspace => {
                    p.filter.pop();
                    p.refilter();
                }
                KeyCode::Char(c) => {
                    p.filter.push(c);
                    p.refilter();
                }
                _ => {}
            }
            continue;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return None,
            KeyCode::Enter => return Some(p.checked.clone()),
            KeyCode::Up | KeyCode::Char('k') => p.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => p.move_by(1),
            KeyCode::PageUp => p.move_by(-10),
            KeyCode::PageDown => p.move_by(10),
            KeyCode::Home | KeyCode::Char('g') => p.move_by(isize::MIN / 2),
            KeyCode::End | KeyCode::Char('G') => p.move_by(isize::MAX / 2),
            KeyCode::Char(' ') => {
                if let Some(i) = p.current() {
                    p.checked[i] = !p.checked[i];
                    p.move_by(1);
                }
            }
            KeyCode::Char('a') => {
                for &i in &p.visible {
                    p.checked[i] = true;
                }
            }
            KeyCode::Char('n') => {
                for &i in &p.visible {
                    p.checked[i] = false;
                }
            }
            KeyCode::Char('/') => p.typing = true,
            _ => {}
        }
    }
}

fn draw_picker(f: &mut Frame, p: &mut Picker) {
    let [head, body, detail, foot] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(5),
        Constraint::Length(1),
    ])
    .areas(f.area());

    let selected = p.checked.iter().filter(|c| **c).count();
    let mut title = vec![
        Span::styled(copy::TUI_TITLE, Style::new().bold()),
        Span::raw(format!(
            "  {}",
            fill(
                copy::TUI_COUNT,
                &[&selected.to_string(), &p.items.len().to_string()]
            )
        )),
    ];
    if p.typing || !p.filter.is_empty() {
        title.push(Span::raw(format!("   {}: {}", copy::TUI_FILTER, p.filter)));
        if p.typing {
            title.push(Span::raw("_").slow_blink());
        }
    }
    f.render_widget(Line::from(title), head);

    let header = Row::new(
        copy::TUI_COLUMNS
            .iter()
            .map(|h| Cell::from(*h).style(Style::new().bold().underlined())),
    );
    let rows: Vec<Row> = p
        .visible
        .iter()
        .map(|&i| {
            let it = &p.items[i];
            let age = it.age();
            let age_style = age_color(age.level())
                .map(|c| Style::new().fg(c))
                .unwrap_or_default();
            let mut name = it.name.clone();
            if it.electron {
                name.push_str(&format!(" [{}]", copy::TAG_ELECTRON));
            }
            Row::new(vec![
                Cell::from(if p.checked[i] { "[x]" } else { "[ ]" }),
                Cell::from(name),
                Cell::from(it.path.clone()),
                Cell::from(it.description.clone()),
                Cell::from(it.current.version.clone()),
                Cell::from(date_label(it.current.date)),
                Cell::from(it.latest.version.clone()),
                Cell::from(date_label(it.latest.date)),
                Cell::from(age.label()).style(age_style),
            ])
        })
        .collect();
    let widths = [
        Constraint::Length(3),
        Constraint::Min(16),
        Constraint::Fill(2),
        Constraint::Fill(3),
        Constraint::Length(14),
        Constraint::Length(10),
        Constraint::Length(14),
        Constraint::Length(10),
        Constraint::Length(5),
    ];
    let table = Table::new(rows, widths)
        .header(header)
        .column_spacing(1)
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    f.render_stateful_widget(table, body, &mut p.state);

    draw_detail(f, detail, p.current().map(|i| &p.items[i]));
    f.render_widget(Line::from(copy::TUI_KEYS).style(Style::new().dim()), foot);
}

fn draw_detail(f: &mut Frame, area: Rect, item: Option<&Item>) {
    let Some(it) = item else {
        f.render_widget(Paragraph::new(copy::TUI_NO_MATCH), area);
        return;
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled(it.name.clone(), Style::new().bold()),
            Span::raw(format!("  {}", it.id)),
        ]),
        Line::from(it.path.clone()),
        Line::from(it.description.clone()),
    ];
    let mut extra = Vec::new();
    if let Some(note) = &it.note {
        extra.push(note.clone());
    }
    if it.needs_restart {
        extra.push(copy::TUI_NEEDS_RESTART.to_string());
    }
    if !extra.is_empty() {
        lines.push(Line::from(extra.join("  ")).style(Style::new().fg(Color::Cyan)));
    }
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(Block::bordered()),
        area,
    );
}

/// The first-run screen for shell frameworks. Returns the IDs the user turned
/// on, or None when they leave without saving.
pub fn shell_screen(found: &[Found], enabled: &[String]) -> Option<Vec<String>> {
    let mut checked: Vec<bool> = found
        .iter()
        .map(|f| enabled.iter().any(|e| e == f.id))
        .collect();
    let mut state = TableState::default().with_selected(Some(0));
    let mut terminal = ratatui::init();
    let result = loop {
        let drawn = terminal.draw(|f| {
            let [intro, body, detail, foot] = Layout::vertical([
                Constraint::Length(4),
                Constraint::Fill(1),
                Constraint::Length(5),
                Constraint::Length(1),
            ])
            .areas(f.area());
            f.render_widget(
                Paragraph::new(vec![
                    Line::from(copy::SHELL_TITLE).bold(),
                    Line::from(copy::SHELL_INTRO),
                ])
                .wrap(Wrap { trim: true }),
                intro,
            );
            let rows = found.iter().zip(&checked).map(|(fw, on)| {
                Row::new(vec![
                    Cell::from(if *on { "[x]" } else { "[ ]" }),
                    Cell::from(fw.name),
                    Cell::from(fw.root.display().to_string()),
                ])
            });
            let table = Table::new(
                rows,
                [
                    Constraint::Length(3),
                    Constraint::Min(24),
                    Constraint::Fill(1),
                ],
            )
            .header(Row::new(copy::SHELL_COLUMNS.iter().copied()).bold())
            .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED));
            f.render_stateful_widget(table, body, &mut state);
            if let Some(fw) = state.selected().and_then(|i| found.get(i)) {
                let mut lines = vec![Line::from(fill(copy::SHELL_RUNS, &[fw.update_command()]))];
                if let Some(w) = fw.warning() {
                    lines.push(Line::from(w).style(Style::new().fg(Color::Yellow)));
                }
                f.render_widget(
                    Paragraph::new(lines)
                        .wrap(Wrap { trim: true })
                        .block(Block::bordered()),
                    detail,
                );
            }
            f.render_widget(Line::from(copy::SHELL_KEYS).style(Style::new().dim()), foot);
        });
        if drawn.is_err() {
            break None;
        }
        let Ok(Event::Key(key)) = event::read() else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        let sel = state.selected().unwrap_or(0);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => break None,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break None,
            KeyCode::Enter => {
                break Some(
                    found
                        .iter()
                        .zip(&checked)
                        .filter(|(_, on)| **on)
                        .map(|(fw, _)| fw.id.to_string())
                        .collect(),
                );
            }
            KeyCode::Up | KeyCode::Char('k') => state.select(Some(sel.saturating_sub(1))),
            KeyCode::Down | KeyCode::Char('j') => {
                state.select(Some((sel + 1).min(found.len().saturating_sub(1))))
            }
            KeyCode::Char(' ') => {
                if let Some(c) = checked.get_mut(sel) {
                    *c = !*c;
                }
            }
            _ => {}
        }
    };
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Action, Release, Source};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn item(name: &str, from: &str, to: &str) -> Item {
        Item::new(
            Source::Brew,
            format!("brew:{name}"),
            name,
            Action::BrewFormula(name.into()),
        )
        .path(format!("/opt/homebrew/opt/{name}"))
        .description("test item")
        .versions(
            Release::new("1.0", Some(from.parse().unwrap())),
            Release::new("2.0", Some(to.parse().unwrap())),
        )
    }

    /// Renders the picker off screen and checks the rows, their order, and
    /// the age colors.
    #[test]
    fn picker_renders_and_colors_ages() {
        let items = vec![
            item("fresh", "2026-10-01T00:00:00Z", "2026-10-03T00:00:00Z"),
            item("stale", "2026-08-01T00:00:00Z", "2026-10-01T00:00:00Z"),
            item("weekold", "2026-10-01T00:00:00Z", "2026-10-09T00:00:00Z"),
        ];
        let mut order: Vec<usize> = (0..items.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(items[i].age().days().unwrap_or(-1)));
        let mut picker = Picker {
            items: &items,
            checked: vec![true, false, true],
            visible: order.clone(),
            order,
            state: TableState::default().with_selected(Some(0)),
            filter: String::new(),
            typing: false,
        };
        let mut terminal = Terminal::new(TestBackend::new(160, 20)).unwrap();
        terminal.draw(|f| draw_picker(f, &mut picker)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let lines: Vec<String> = (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect();
        let row = |name: &str| lines.iter().position(|l| l.contains(name)).unwrap();
        // Most out of date first.
        assert!(row("stale") < row("weekold") && row("weekold") < row("fresh"));
        assert!(lines[row("fresh")].contains("[x]"));
        assert!(lines[row("stale")].contains("[ ]"));
        assert!(lines[0].contains("2 of 3 selected"));
        let color_of = |name: &str, days: &str| {
            let y = row(name);
            let x = lines[y].rfind(days).unwrap();
            buf[(x as u16, y as u16)].fg
        };
        assert_eq!(color_of("stale", "61"), Color::Red);
        assert_eq!(color_of("weekold", "8"), Color::Yellow);
    }
}
