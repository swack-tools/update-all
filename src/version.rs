//! Loose version comparison that copes with the many formats apps use.
//!
//! The rules follow Sparkle's standard comparator: numbers compare as numbers,
//! words compare as text, and a release beats a prerelease with the same
//! numbers, so `1.0` is newer than `1.0b1`.

use std::cmp::Ordering;

#[derive(Debug, PartialEq, Eq)]
enum Part<'a> {
    Num(u64),
    Word(&'a str),
}

/// Drops prefixes like `v` or `Build ` and anything after a comma, which
/// Homebrew casks use for build IDs.
pub fn clean(raw: &str) -> &str {
    let raw = raw.split(',').next().unwrap_or("").trim();
    match raw.find(|c: char| c.is_ascii_digit()) {
        Some(start) => &raw[start..],
        None => raw,
    }
}

fn parts(raw: &str) -> Vec<Part<'_>> {
    let mut out = Vec::new();
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            // Very long digit runs, such as hashes, saturate instead of overflowing.
            out.push(Part::Num(raw[start..i].parse().unwrap_or(u64::MAX)));
        } else if c.is_ascii_alphabetic() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                i += 1;
            }
            out.push(Part::Word(&raw[start..i]));
        } else {
            i += 1;
        }
    }
    out
}

/// Compares two version strings.
pub fn compare(a: &str, b: &str) -> Ordering {
    let (pa, pb) = (parts(clean(a)), parts(clean(b)));
    for i in 0..pa.len().max(pb.len()) {
        let ord = match (pa.get(i), pb.get(i)) {
            (Some(Part::Num(x)), Some(Part::Num(y))) => x.cmp(y),
            (Some(Part::Word(x)), Some(Part::Word(y))) => {
                x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase())
            }
            // A number beats a word in the same spot: 1.0.1 > 1.0b.
            (Some(Part::Num(_)), Some(Part::Word(_))) => Ordering::Greater,
            (Some(Part::Word(_)), Some(Part::Num(_))) => Ordering::Less,
            // When one side runs out, a trailing word marks a prerelease
            // and a trailing nonzero number marks a newer release.
            (Some(Part::Word(_)), None) => Ordering::Less,
            (None, Some(Part::Word(_))) => Ordering::Greater,
            (Some(Part::Num(x)), None) => x.cmp(&0),
            (None, Some(Part::Num(y))) => 0.cmp(y),
            (None, None) => Ordering::Equal,
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    Ordering::Equal
}

/// True when the first version is newer than the second. Returns false when either side
/// has no digits at all, because then the comparison means nothing.
pub fn is_newer(latest: &str, current: &str) -> bool {
    has_digit(latest) && has_digit(current) && compare(latest, current) == Ordering::Greater
}

fn has_digit(s: &str) -> bool {
    s.chars().any(|c| c.is_ascii_digit())
}

/// True when `version` satisfies a pin such as `22` or `1.95.3`.
pub fn matches_pin(version: &str, pin: &str) -> bool {
    let (v, p) = (clean(version), clean(pin));
    v == p
        || v.strip_prefix(p)
            .is_some_and(|rest| rest.starts_with(['.', '-', '_', '+']))
}

/// Pulls the first version-looking token out of command output such as
/// `codex-cli 0.165.0` or `2.1.312 Claude Code`.
pub fn find_in(text: &str) -> Option<String> {
    text.split(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == ',')
        .map(|w| w.trim_start_matches('v'))
        .find(|w| {
            w.chars().next().is_some_and(|c| c.is_ascii_digit())
                && w.contains('.')
                && w.chars()
                    .all(|c| c.is_ascii_alphanumeric() || ".-_+".contains(c))
        })
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_order() {
        assert!(is_newer("1.10.0", "1.9.9"));
        assert!(is_newer("2.0", "1.99.99"));
        assert!(!is_newer("1.0.0", "1.0"));
        assert!(is_newer("1.0.1", "1.0"));
    }

    #[test]
    fn prereleases() {
        assert!(is_newer("1.0", "1.0b1"));
        assert!(is_newer("1.0b2", "1.0b1"));
        assert!(is_newer("8.12.44-25.BETA", "8.12.42"));
        assert!(!is_newer("8.12.42", "8.12.44-25.BETA"));
    }

    #[test]
    fn messy_formats() {
        assert!(!is_newer("4215", "Build 4215"));
        assert!(is_newer("4.29.1,abc123", "4.29.0"));
        assert!(!is_newer("21.3", "21.3."));
        assert!(is_newer("v0.166.0", "0.165.0"));
        assert!(!is_newer("latest", "1.0"));
    }

    #[test]
    fn pins() {
        assert!(matches_pin("26.12.1", "26"));
        assert!(!matches_pin("260.1", "26"));
        assert!(matches_pin("1.95.3", "1.95.3"));
        assert!(!matches_pin("1.96.0", "1.95"));
    }

    #[test]
    fn finds_versions_in_output() {
        assert_eq!(find_in("codex-cli 0.165.0").as_deref(), Some("0.165.0"));
        assert_eq!(find_in("2.1.312 (Claude Code)").as_deref(), Some("2.1.312"));
        assert_eq!(find_in("ripgrep v14.1.1").as_deref(), Some("14.1.1"));
        assert_eq!(find_in("no version here"), None);
    }
}
