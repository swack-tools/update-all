//! Every place update-all looks for updates. Each source checks only when its
//! tool exists and stays silent otherwise.

pub mod ai;
pub mod apps;
pub mod appstore;
pub mod brew;
pub mod catalog;
mod commerce;
pub mod devtools;
pub mod docker;
pub mod helm;
pub mod langs;
pub mod macos;
pub mod shells;
pub mod sparkle;

use std::sync::Mutex;
use std::thread;

use crate::config::Config;
use crate::model::Discovery;

/// What every source can see while it checks.
pub struct Ctx<'a> {
    pub config: &'a Config,
    /// Check every app, not just the known ones.
    pub literally_all: bool,
    /// Shell framework IDs the user chose to update.
    pub shells_enabled: &'a [String],
}

pub type Discover = fn(&Ctx) -> Discovery;

/// A named check, run in parallel with the others.
pub struct Check {
    pub name: &'static str,
    pub run: Discover,
}

pub const CHECKS: &[Check] = &[
    Check {
        name: crate::copy::LABEL_BREW,
        run: brew::discover,
    },
    Check {
        name: crate::copy::LABEL_LANGS,
        run: langs::discover,
    },
    Check {
        name: crate::copy::LABEL_AI,
        run: ai::discover,
    },
    Check {
        name: crate::copy::LABEL_APPS_AND_BROWSERS,
        run: apps::discover,
    },
    Check {
        name: crate::copy::LABEL_APP_STORE,
        run: appstore::discover,
    },
    Check {
        name: crate::copy::LABEL_DEVTOOLS,
        run: devtools::discover,
    },
    Check {
        name: crate::copy::LABEL_DOCKER,
        run: docker::discover,
    },
    Check {
        name: crate::copy::LABEL_HELM,
        run: helm::discover,
    },
    Check {
        name: crate::copy::LABEL_SHELLS,
        run: shells::discover,
    },
    Check {
        name: crate::copy::LABEL_MACOS,
        run: macos::discover,
    },
];

/// Descriptions from the built-in registries, for the Vale check.
#[cfg(test)]
pub fn described() -> Vec<(String, &'static str)> {
    let mut all = apps::described();
    all.extend(ai::described());
    all
}

/// Runs `f` over `inputs` on up to `workers` threads and keeps the input order.
pub fn par_map<T, R, F>(inputs: Vec<T>, workers: usize, f: F) -> Vec<R>
where
    T: Send,
    R: Send,
    F: Fn(T) -> R + Sync,
{
    let total = inputs.len();
    let queue = Mutex::new(inputs.into_iter().enumerate());
    let results = Mutex::new(Vec::with_capacity(total));
    thread::scope(|scope| {
        for _ in 0..workers.clamp(1, total.max(1)) {
            scope.spawn(|| {
                loop {
                    let next = queue.lock().ok().and_then(|mut q| q.next());
                    let Some((idx, input)) = next else { break };
                    let out = f(input);
                    if let Ok(mut r) = results.lock() {
                        r.push((idx, out));
                    }
                }
            });
        }
    });
    let mut results = results.into_inner().unwrap_or_default();
    results.sort_by_key(|(idx, _)| *idx);
    results.into_iter().map(|(_, r)| r).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn par_map_keeps_order() {
        let out = par_map((0..50).collect(), 8, |x: i32| x * 2);
        assert_eq!(out, (0..50).map(|x| x * 2).collect::<Vec<_>>());
    }
}
