//! Shared types: the items that sources discover and the actions that update them.

use jiff::Timestamp;

/// Where an item comes from. The order here is the order in which updates run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Source {
    Brew,
    Langs,
    Ai,
    Apps,
    AppStore,
    DevTools,
    Docker,
    Shells,
    MacOs,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Brew => crate::copy::LABEL_BREW,
            Source::Langs => crate::copy::LABEL_LANGS,
            Source::Ai => crate::copy::LABEL_AI,
            Source::Apps => crate::copy::LABEL_APPS,
            Source::AppStore => crate::copy::LABEL_APP_STORE,
            Source::DevTools => crate::copy::LABEL_DEVTOOLS,
            Source::Docker => crate::copy::LABEL_DOCKER,
            Source::Shells => crate::copy::LABEL_SHELLS,
            Source::MacOs => crate::copy::LABEL_MACOS,
        }
    }
}

/// A version and its release date, when known.
#[derive(Clone, Debug, Default)]
pub struct Release {
    pub version: String,
    pub date: Option<Timestamp>,
}

impl Release {
    pub fn new(version: impl Into<String>, date: Option<Timestamp>) -> Self {
        Release {
            version: version.into(),
            date,
        }
    }
}

/// How an item gets updated.
#[derive(Clone, Debug)]
pub enum Action {
    /// Runs one command. `sudo` asks for administrator rights first.
    Command {
        program: String,
        args: Vec<String>,
        sudo: bool,
    },
    /// Batched into one `brew upgrade --formula` call.
    BrewFormula(String),
    /// Batched into one `brew upgrade --cask` call.
    BrewCask(String),
    /// Downloads and installs an update from a Sparkle feed.
    Sparkle(crate::sources::sparkle::Update),
    /// Pulls a Docker image tag.
    DockerPull(String),
    /// Opens the app hidden in the background so its own updater runs.
    LaunchHidden { app: String },
    /// Updates an App Store app.
    AppStore { adam_id: u64, app: String },
    /// Installs one `softwareupdate` label.
    SoftwareUpdate { label: String },
}

/// Something update-all found that has a newer version.
#[derive(Clone, Debug)]
pub struct Item {
    /// Stable ID, such as `brew:node` or `app:Claude`. Config patterns match it.
    pub id: String,
    pub name: String,
    pub source: Source,
    /// Binary or app path shown in the picker.
    pub path: String,
    pub description: String,
    pub current: Release,
    pub latest: Release,
    pub action: Action,
    /// The app uses Electron. Electron apps are opt-in.
    pub electron: bool,
    /// The update needs a restart, so it runs last.
    pub needs_restart: bool,
    /// A short note shown next to the item, such as why it's held back.
    pub note: Option<String>,
    /// Set when a config pin holds the item back. Held items never update.
    pub held: bool,
}

impl Item {
    pub fn new(
        source: Source,
        id: impl Into<String>,
        name: impl Into<String>,
        action: Action,
    ) -> Self {
        Item {
            id: id.into(),
            name: name.into(),
            source,
            path: String::new(),
            description: String::new(),
            current: Release::default(),
            latest: Release::default(),
            action,
            electron: false,
            needs_restart: false,
            note: None,
            held: false,
        }
    }

    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = one_line(&description.into());
        self
    }

    pub fn versions(mut self, current: Release, latest: Release) -> Self {
        self.current = current;
        self.latest = latest;
        self
    }

    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// True for items in the opt-in groups.
    pub fn app_store(&self) -> bool {
        self.source == Source::AppStore
    }

    pub fn age(&self) -> Age {
        Age::between(&self.current, &self.latest, Timestamp::now())
    }
}

/// How far behind an item is, in days.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Age {
    /// update-all knows both release dates.
    Exact(i64),
    /// update-all knows only the newest release date, so this counts the days
    /// since that release, a lower bound.
    AtLeast(i64),
    Unknown,
}

impl Age {
    pub fn between(current: &Release, latest: &Release, now: Timestamp) -> Age {
        match (current.date, latest.date) {
            (Some(c), Some(l)) => Age::Exact(days(c, l)),
            (None, Some(l)) => Age::AtLeast(days(l, now)),
            _ => Age::Unknown,
        }
    }

    pub fn days(self) -> Option<i64> {
        match self {
            Age::Exact(d) | Age::AtLeast(d) => Some(d),
            Age::Unknown => None,
        }
    }

    pub fn label(self) -> String {
        match self {
            Age::Exact(d) => d.to_string(),
            Age::AtLeast(d) => format!("~{d}"),
            Age::Unknown => "-".to_string(),
        }
    }

    pub fn level(self) -> AgeLevel {
        match self.days() {
            Some(d) if d >= 30 => AgeLevel::Red,
            Some(d) if d >= 14 => AgeLevel::Orange,
            Some(d) if d >= 7 => AgeLevel::Yellow,
            _ => AgeLevel::Plain,
        }
    }
}

/// The color for an age: yellow at 7 days or more, orange at 14, red at 30.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgeLevel {
    Plain,
    Yellow,
    Orange,
    Red,
}

fn days(from: Timestamp, to: Timestamp) -> i64 {
    ((to.as_second() - from.as_second()) / 86_400).max(0)
}

/// Formats a release date like 2026-10-28, or a dash when unknown.
pub fn date_label(date: Option<Timestamp>) -> String {
    match date {
        Some(ts) => ts.strftime("%Y-%m-%d").to_string(),
        None => "-".to_string(),
    }
}

/// Collapses whitespace so a description fits on one line.
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A block of information that update-all shows but never acts on.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub title: String,
    pub lines: Vec<String>,
}

/// Everything one source found.
#[derive(Debug, Default)]
pub struct Discovery {
    pub items: Vec<Item>,
    pub reports: Vec<Report>,
    pub notes: Vec<String>,
    /// Set when some checks failed, so "up to date" would overstate things.
    pub incomplete: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    #[test]
    fn age_uses_both_dates_when_known() {
        let cur = Release::new("1.0", Some(ts("2026-01-01T00:00:00Z")));
        let new = Release::new("2.0", Some(ts("2026-01-15T00:00:00Z")));
        let age = Age::between(&cur, &new, ts("2026-03-01T00:00:00Z"));
        assert_eq!(age, Age::Exact(14));
        assert_eq!(age.level(), AgeLevel::Orange);
    }

    #[test]
    fn age_falls_back_to_lower_bound() {
        let cur = Release::new("1.0", None);
        let new = Release::new("2.0", Some(ts("2026-01-01T00:00:00Z")));
        let age = Age::between(&cur, &new, ts("2026-02-15T00:00:00Z"));
        assert_eq!(age, Age::AtLeast(45));
        assert_eq!(age.label(), "~45");
        assert_eq!(age.level(), AgeLevel::Red);
    }

    #[test]
    fn age_thresholds() {
        let level = |d| Age::Exact(d).level();
        assert_eq!(level(6), AgeLevel::Plain);
        assert_eq!(level(7), AgeLevel::Yellow);
        assert_eq!(level(13), AgeLevel::Yellow);
        assert_eq!(level(14), AgeLevel::Orange);
        assert_eq!(level(29), AgeLevel::Orange);
        assert_eq!(level(30), AgeLevel::Red);
        assert_eq!(Age::Unknown.level(), AgeLevel::Plain);
    }

    #[test]
    fn age_never_goes_negative() {
        let cur = Release::new("2.0", Some(ts("2026-02-01T00:00:00Z")));
        let new = Release::new("2.1", Some(ts("2026-01-01T00:00:00Z")));
        assert_eq!(Age::between(&cur, &new, Timestamp::now()), Age::Exact(0));
    }
}
