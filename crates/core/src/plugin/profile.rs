// SPDX-License-Identifier: MIT OR Apache-2.0
//! Per-plugin startup and call timings.
//!
//! Profiling is off unless Forge is started with `--profile-startup`, a
//! plugin startup budget is set, or `M-x plugin-profile` turns it on. While
//! it is off every hot path only checks one atomic boolean: no clock is read
//! and no allocation or lock happens. A [`Clock`] is behind the timings so
//! tests can advance time explicitly and compare the rendered report byte for
//! byte.

use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The source of monotonic time for profiling.
pub trait Clock: Send + Sync {
    /// Time since an arbitrary origin. Only differences are meaningful.
    fn now(&self) -> Duration;
}

/// The production clock: monotonic time since it was created.
#[derive(Debug)]
pub struct SystemClock {
    origin: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

/// A clock tests move by hand.
#[derive(Debug, Default)]
pub struct ManualClock {
    now_ns: AtomicU64,
}

impl ManualClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// Move the clock forward.
    pub fn advance(&self, by: Duration) {
        self.now_ns.fetch_add(ns(by), Ordering::SeqCst);
    }

    /// Set the clock to an absolute time since its origin.
    pub fn set(&self, at: Duration) {
        self.now_ns.store(ns(at), Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Duration {
        Duration::from_nanos(self.now_ns.load(Ordering::SeqCst))
    }
}

/// What one category of work cost: how often it ran, total time, longest run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Stat {
    pub count: u64,
    pub total_ns: u64,
    pub max_ns: u64,
}

impl Stat {
    fn record(&mut self, elapsed: Duration) {
        let elapsed = ns(elapsed);
        self.count = self.count.saturating_add(1);
        self.total_ns = self.total_ns.saturating_add(elapsed);
        self.max_ns = self.max_ns.max(elapsed);
    }

    pub fn total(&self) -> Duration {
        Duration::from_nanos(self.total_ns)
    }

    pub fn max(&self) -> Duration {
        Duration::from_nanos(self.max_ns)
    }
}

/// One plugin's timings, as shown in the report.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PluginProfile {
    pub plugin: String,
    pub cache_lookup: Stat,
    pub compile: Stat,
    pub init: Stat,
    pub lazy_start: Stat,
    /// Each command, event and callback, by name.
    pub calls: BTreeMap<String, Stat>,
}

/// Everything a report shows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ProfileReport {
    /// Time from the start of plugin loading, if profiling covered it.
    pub startup_ns: Option<u64>,
    /// `forge.opt["startup_budget_ms"]`, if set.
    pub budget_ns: Option<u64>,
    pub over_budget: bool,
    pub plugins: Vec<PluginProfile>,
}

impl ProfileReport {
    pub fn startup(&self) -> Option<Duration> {
        self.startup_ns.map(Duration::from_nanos)
    }

    pub fn budget(&self) -> Option<Duration> {
        self.budget_ns.map(Duration::from_nanos)
    }

    /// A stable, human-readable report. Tests compare this exactly.
    pub fn render_text(&self) -> String {
        let mut out = String::from("Plugin startup profile\n");
        match self.startup() {
            Some(total) => out.push_str(&format!("startup: {} ms\n", millis(total))),
            None => out.push_str("startup: not measured\n"),
        }
        if let Some(budget) = self.budget() {
            if self.over_budget {
                let over = self.startup().unwrap_or_default().saturating_sub(budget);
                out.push_str(&format!(
                    "budget: {} ms (over by {} ms)\n",
                    millis(budget),
                    millis(over)
                ));
            } else {
                out.push_str(&format!("budget: {} ms\n", millis(budget)));
            }
        }
        if self.plugins.is_empty() {
            out.push_str("no plugin timings recorded\n");
            return out;
        }
        for plugin in &self.plugins {
            out.push_str(&format!("\n{}\n", plugin.plugin));
            stat_line(&mut out, "cache lookup", &plugin.cache_lookup);
            stat_line(&mut out, "compile", &plugin.compile);
            stat_line(&mut out, "init", &plugin.init);
            stat_line(&mut out, "lazy start", &plugin.lazy_start);
            if !plugin.calls.is_empty() {
                out.push_str("  calls:\n");
                for (label, stat) in &plugin.calls {
                    stat_line_indented(&mut out, "    ", label, stat);
                }
            }
        }
        out
    }
}

fn stat_line(out: &mut String, label: &str, stat: &Stat) {
    stat_line_indented(out, "  ", label, stat);
}

fn stat_line_indented(out: &mut String, indent: &str, label: &str, stat: &Stat) {
    if stat.count == 0 {
        return;
    }
    let calls = if stat.count == 1 { "call" } else { "calls" };
    out.push_str(&format!(
        "{indent}{label}: {} {calls}, total {} ms, max {} ms\n",
        stat.count,
        millis(stat.total()),
        millis(stat.max())
    ));
}

fn millis(d: Duration) -> String {
    format!("{:.3}", d.as_secs_f64() * 1000.0)
}

fn ns(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}

const UNSET: u64 = u64::MAX;

/// Which part of plugin work a timer measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Part {
    CacheLookup,
    Compile,
    Init,
    LazyStart,
    Call,
}

#[derive(Debug, Clone, Default)]
struct Timings {
    cache_lookup: Stat,
    compile: Stat,
    init: Stat,
    lazy_start: Stat,
    calls: BTreeMap<String, Stat>,
}

impl Timings {
    fn add(&mut self, part: Part, label: &str, elapsed: Duration) {
        match part {
            Part::CacheLookup => self.cache_lookup.record(elapsed),
            Part::Compile => self.compile.record(elapsed),
            Part::Init => self.init.record(elapsed),
            Part::LazyStart => self.lazy_start.record(elapsed),
            Part::Call => self
                .calls
                .entry(label.to_string())
                .or_default()
                .record(elapsed),
        }
    }
}

struct Inner {
    enabled: AtomicBool,
    clock: Arc<dyn Clock>,
    timings: Mutex<BTreeMap<String, Timings>>,
    startup_start: AtomicU64,
    startup_end: AtomicU64,
}

/// The shared profile recorder. Cloning is cheap and safe to move to a
/// plugin's background thread.
#[derive(Clone)]
pub struct Profiler {
    inner: Arc<Inner>,
}

impl Default for Profiler {
    fn default() -> Self {
        Self::new()
    }
}

impl Profiler {
    pub fn new() -> Self {
        Self::with_clock(Arc::new(SystemClock::new()))
    }

    pub fn with_clock(clock: Arc<dyn Clock>) -> Self {
        Self {
            inner: Arc::new(Inner {
                enabled: AtomicBool::new(false),
                clock,
                timings: Mutex::new(BTreeMap::new()),
                startup_start: AtomicU64::new(UNSET),
                startup_end: AtomicU64::new(UNSET),
            }),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.inner.enabled.load(Ordering::Relaxed)
    }

    /// Turn profiling on. Turning it on after it was off starts a fresh
    /// report, so old data from another run is not mixed in.
    pub fn enable(&self) {
        if !self.inner.enabled.swap(true, Ordering::SeqCst) {
            let mut timings = self.lock();
            timings.clear();
            self.inner.startup_start.store(UNSET, Ordering::SeqCst);
            self.inner.startup_end.store(UNSET, Ordering::SeqCst);
        }
    }

    /// Start measuring startup, if profiling is on.
    pub fn begin_startup(&self) {
        if self.is_enabled() {
            self.inner
                .startup_start
                .store(self.now_ns(), Ordering::SeqCst);
            self.inner.startup_end.store(UNSET, Ordering::SeqCst);
        }
    }

    /// Stop measuring startup, if profiling is on.
    pub fn finish_startup(&self) {
        if self.is_enabled() {
            self.inner
                .startup_end
                .store(self.now_ns(), Ordering::SeqCst);
        }
    }

    pub fn startup_total(&self) -> Option<Duration> {
        let start = self.inner.startup_start.load(Ordering::SeqCst);
        let end = self.inner.startup_end.load(Ordering::SeqCst);
        (start != UNSET && end != UNSET).then(|| Duration::from_nanos(end.saturating_sub(start)))
    }

    /// A label with detail, or an empty string when profiling is off (so
    /// the formatting work is skipped entirely).
    pub(crate) fn label(&self, prefix: &str, detail: &str) -> String {
        if self.is_enabled() {
            format!("{prefix}:{detail}")
        } else {
            String::new()
        }
    }

    pub(crate) fn timer(&self, plugin: &str, part: Part, label: &str) -> Option<Timer> {
        if !self.is_enabled() {
            return None;
        }
        Some(Timer {
            profiler: self.clone(),
            plugin: plugin.to_string(),
            part,
            label: label.to_string(),
            start_ns: self.now_ns(),
        })
    }

    /// The report for everything recorded so far.
    pub fn report(&self, budget: Option<Duration>) -> ProfileReport {
        let startup_ns = self.startup_total().map(ns);
        let budget_ns = budget.map(ns);
        let over_budget = match (startup_ns, budget_ns) {
            (Some(total), Some(budget)) => total > budget,
            _ => false,
        };
        let plugins = self
            .lock()
            .iter()
            .map(|(plugin, t)| PluginProfile {
                plugin: plugin.clone(),
                cache_lookup: t.cache_lookup,
                compile: t.compile,
                init: t.init,
                lazy_start: t.lazy_start,
                calls: t.calls.clone(),
            })
            .collect();
        ProfileReport {
            startup_ns,
            budget_ns,
            over_budget,
            plugins,
        }
    }

    fn now_ns(&self) -> u64 {
        ns(self.inner.clock.now())
    }

    fn record(&self, plugin: &str, part: Part, label: &str, elapsed: Duration) {
        self.lock()
            .entry(plugin.to_string())
            .or_default()
            .add(part, label, elapsed);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Timings>> {
        match self.inner.timings.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// Times one unit of work until it is dropped. Created only while
/// profiling is on.
pub(crate) struct Timer {
    profiler: Profiler,
    plugin: String,
    part: Part,
    label: String,
    start_ns: u64,
}

impl Timer {
    /// Reclassify work once its outcome is known (a cache hit vs a compile).
    pub(crate) fn set_part(&mut self, part: Part) {
        self.part = part;
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        let elapsed = Duration::from_nanos(self.profiler.now_ns().saturating_sub(self.start_ns));
        self.profiler
            .record(&self.plugin, self.part, &self.label, elapsed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manual_clock_makes_the_report_deterministic() {
        let clock = Arc::new(ManualClock::new());
        let profiler = Profiler::with_clock(clock.clone());
        profiler.enable();
        profiler.begin_startup();

        {
            let mut timer = profiler
                .timer("demo", Part::Compile, "")
                .expect("profiling is on");
            clock.advance(Duration::from_millis(4));
            timer.set_part(Part::Compile);
        }
        {
            let _timer = profiler.timer("demo", Part::Init, "").expect("on");
            clock.advance(Duration::from_millis(2));
        }
        for (label, ms) in [("command:demo-greet", 3), ("command:demo-greet", 5)] {
            let _timer = profiler.timer("demo", Part::Call, label).expect("on");
            clock.advance(Duration::from_millis(ms));
        }
        clock.advance(Duration::from_millis(1));
        profiler.finish_startup();

        let report = profiler.report(Some(Duration::from_millis(10)));
        assert_eq!(report.startup(), Some(Duration::from_millis(15)));
        assert!(report.over_budget);
        assert_eq!(report.plugins.len(), 1);
        assert_eq!(report.plugins[0].compile.count, 1);
        assert_eq!(report.plugins[0].init.total(), Duration::from_millis(2));
        let calls = &report.plugins[0].calls["command:demo-greet"];
        assert_eq!(calls.count, 2);
        assert_eq!(calls.total(), Duration::from_millis(8));
        assert_eq!(calls.max(), Duration::from_millis(5));

        assert_eq!(
            report.render_text(),
            "Plugin startup profile\n\
             startup: 15.000 ms\n\
             budget: 10.000 ms (over by 5.000 ms)\n\
             \n\
             demo\n\
             \x20 compile: 1 call, total 4.000 ms, max 4.000 ms\n\
             \x20 init: 1 call, total 2.000 ms, max 2.000 ms\n\
             \x20 calls:\n\
             \x20   command:demo-greet: 2 calls, total 8.000 ms, max 5.000 ms\n"
        );
    }

    #[test]
    fn profiling_off_does_not_allocate_a_timer() {
        let clock = Arc::new(ManualClock::new());
        let profiler = Profiler::with_clock(clock);
        assert!(profiler.timer("demo", Part::Call, "command:x").is_none());
        assert!(profiler.report(None).plugins.is_empty());
    }

    #[test]
    fn json_has_counts_and_nanoseconds() {
        let clock = Arc::new(ManualClock::new());
        let profiler = Profiler::with_clock(clock.clone());
        profiler.enable();
        let _timer = profiler.timer("demo", Part::CacheLookup, "").expect("on");
        clock.advance(Duration::from_micros(250));
        drop(_timer);
        let value = serde_json::to_value(profiler.report(None)).expect("serializable");
        assert_eq!(value["plugins"][0]["plugin"], "demo");
        assert_eq!(value["plugins"][0]["cache_lookup"]["count"], 1);
        assert_eq!(value["plugins"][0]["cache_lookup"]["total_ns"], 250_000);
    }
}
