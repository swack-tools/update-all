//! Runs commands: quietly with a timeout during discovery, and live during updates.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

/// Folders searched for tools in addition to PATH. These are the normal
/// install locations, so update-all finds tools even when PATH is short.
const EXTRA_BIN_DIRS: &[&str] = &[
    "/opt/homebrew/bin",
    "/opt/homebrew/sbin",
    "/usr/local/bin",
    "~/.local/bin",
    "~/.cargo/bin",
    "~/.bun/bin",
    "~/.deno/bin",
    "~/.lmstudio/bin",
    "~/.opencode/bin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
];

static SEARCH_PATH: OnceLock<OsString> = OnceLock::new();
static LOG: Mutex<Option<File>> = Mutex::new(None);

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Expands a leading `~/` to the home folder.
pub fn expand(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None if path == "~" => home(),
        None => PathBuf::from(path),
    }
}

/// PATH plus the usual install folders, without duplicates.
pub fn search_path() -> &'static OsString {
    SEARCH_PATH.get_or_init(|| {
        let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        for extra in EXTRA_BIN_DIRS {
            let dir = expand(extra);
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
        std::env::join_paths(dirs).unwrap_or_default()
    })
}

/// Finds a program the way a shell would, using [`search_path`].
pub fn which(program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        let p = expand(program);
        return is_executable(&p).then_some(p);
    }
    std::env::split_paths(search_path())
        .map(|dir| dir.join(program))
        .find(|p| is_executable(p))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

pub fn has(program: &str) -> bool {
    which(program).is_some()
}

/// True only when `program` is on the PATH that update-all started with,
/// ignoring the extra install folders that [`which`] also searches.
pub fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|dir| is_executable(&dir.join(program))))
}

/// The result of a quiet command.
#[derive(Debug, Default)]
pub struct Output {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }
}

/// Runs a command with no input, captures its output, and stops it after `timeout`.
pub fn run(program: &str, args: &[&str], timeout: Duration) -> Output {
    run_with(program, args, &[], timeout)
}

pub fn run_with(program: &str, args: &[&str], env: &[(&str, &str)], timeout: Duration) -> Output {
    let Some(path) = which(program) else {
        return Output::default();
    };
    let mut cmd = Command::new(&path);
    cmd.args(args)
        .env("PATH", search_path())
        .env("NO_COLOR", "1")
        .env("HOMEBREW_NO_AUTO_UPDATE", "1")
        .env("HOMEBREW_NO_ENV_HINTS", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let started = Instant::now();
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return Output {
                stderr: e.to_string(),
                ..Output::default()
            };
        }
    };
    let out = child.stdout.take().map(reader);
    let err = child.stderr.take().map(reader);
    let mut timed_out = false;
    let code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                timed_out = true;
                break None;
            }
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(_) => break None,
        }
    };
    let output = Output {
        code,
        stdout: out.and_then(|h| h.join().ok()).unwrap_or_default(),
        stderr: err.and_then(|h| h.join().ok()).unwrap_or_default(),
    };
    log(&format!(
        "check  {} -> {:?}{} in {:.1}s",
        display(program, args),
        output.code,
        if timed_out {
            crate::copy::LOG_TIMED_OUT
        } else {
            ""
        },
        started.elapsed().as_secs_f32()
    ));
    output
}

fn reader(mut pipe: impl Read + Send + 'static) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    })
}

/// Runs a command attached to the terminal so its progress and prompts show.
/// Returns true when it exits with status 0.
pub fn run_live(program: &str, args: &[String], sudo: bool) -> bool {
    let started = Instant::now();
    let (bin, all_args): (String, Vec<String>) = if sudo {
        let mut a = vec![program.to_string()];
        a.extend(args.iter().cloned());
        ("sudo".to_string(), a)
    } else {
        (program.to_string(), args.to_vec())
    };
    let Some(path) = which(&bin) else {
        log(&format!(
            "update {} -> {}",
            display_owned(&bin, &all_args),
            crate::copy::LOG_NOT_FOUND
        ));
        return false;
    };
    let status = Command::new(path)
        .args(&all_args)
        .env("PATH", search_path())
        .env("HOMEBREW_NO_ENV_HINTS", "1")
        .status();
    let ok = status.as_ref().is_ok_and(|s| s.success());
    log(&format!(
        "update {} -> {} in {:.1}s",
        display_owned(&bin, &all_args),
        match &status {
            Ok(s) => s.code().map_or("signal".to_string(), |c| c.to_string()),
            Err(e) => e.to_string(),
        },
        started.elapsed().as_secs_f32()
    ));
    ok
}

/// Quotes a command for display.
pub fn display(program: &str, args: &[&str]) -> String {
    std::iter::once(program)
        .chain(args.iter().copied())
        .map(quote)
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn display_owned(program: &str, args: &[String]) -> String {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    display(program, &args)
}

fn quote(word: &str) -> String {
    if !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=@+,%^".contains(c))
    {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

static SUDO_ALIVE: AtomicBool = AtomicBool::new(false);

/// Asks for the administrator password once, then refreshes it in the
/// background until [`sudo_stop`] runs.
pub fn sudo_start() -> bool {
    if SUDO_ALIVE.load(Ordering::SeqCst) {
        return true;
    }
    if !run_live("sudo", &["-v".to_string()], false) {
        return false;
    }
    SUDO_ALIVE.store(true, Ordering::SeqCst);
    thread::spawn(|| {
        while SUDO_ALIVE.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_secs(50));
            let _ = Command::new("/usr/bin/sudo")
                .args(["-n", "-v"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    });
    true
}

pub fn sudo_stop() {
    SUDO_ALIVE.store(false, Ordering::SeqCst);
}

/// Opens the run log in ~/Library/Logs/update-all/.
pub fn open_log() -> Option<PathBuf> {
    let dir = home().join("Library/Logs/update-all");
    fs::create_dir_all(&dir).ok()?;
    let stamp = jiff::Zoned::now().strftime("%Y-%m-%d_%H%M%S").to_string();
    let path = dir.join(format!("{stamp}.log"));
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    *LOG.lock().ok()? = Some(file);
    Some(path)
}

pub fn log(line: &str) {
    if let Ok(mut guard) = LOG.lock()
        && let Some(file) = guard.as_mut()
    {
        let stamp = jiff::Zoned::now().strftime("%H:%M:%S");
        let _ = writeln!(file, "{stamp} {line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_only_when_needed() {
        assert_eq!(display("brew", &["upgrade", "node"]), "brew upgrade node");
        assert_eq!(
            display("open", &["-a", "/Applications/Google Chrome.app"]),
            "open -a '/Applications/Google Chrome.app'"
        );
    }

    #[test]
    fn times_out() {
        let out = run("sleep", &["5"], Duration::from_millis(200));
        assert!(!out.ok());
        assert_eq!(out.code, None);
    }

    #[test]
    fn captures_output() {
        let out = run("echo", &["hello"], Duration::from_secs(5));
        assert!(out.ok());
        assert_eq!(out.stdout.trim(), "hello");
    }
}
