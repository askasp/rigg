
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::{Command, Stdio};

use crate::instance;

/// Days since 1970-01-01 for a proleptic Gregorian date (Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 0 = Sunday, matching cron's own numbering.
fn weekday(days: i64) -> i64 {
    (days + 4).rem_euclid(7)
}

/// A wall-clock minute, as the five things a cron expression looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct When {
    pub year: i64,
    pub month: i64,
    pub day: i64,
    pub hour: i64,
    pub minute: i64,
    pub dow: i64,
}

impl When {
    fn from_days(days: i64, hour: i64, minute: i64) -> Self {
        let (year, month, day) = civil_from_days(days);
        When { year, month, day, hour, minute, dow: weekday(days) }
    }

    fn days(&self) -> i64 {
        days_from_civil(self.year, self.month, self.day)
    }

    pub fn stamp(&self) -> String {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute
        )
    }
}

/// Ask the OS for local wall-clock time, or for the time some phrase names.
pub fn now(at: Option<&str>) -> Result<When> {
    let mut cmd = Command::new("date");
    if let Some(spec) = at {
        cmd.arg("-d").arg(spec);
    }
    let out = cmd
        .arg("+%Y %m %d %H %M %w")
        .output()
        .context("running `date` to read the local clock")?;
    if !out.status.success() {
        bail!(
            "`date` could not read {}",
            at.map(|s| format!("`{s}`")).unwrap_or_else(|| "the clock".into())
        );
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let n: Vec<i64> = text
        .split_whitespace()
        .filter_map(|f| f.parse().ok())
        .collect();
    if n.len() != 6 {
        bail!("could not read a time out of `date` output: {}", text.trim());
    }
    Ok(When { year: n[0], month: n[1], day: n[2], hour: n[3], minute: n[4], dow: n[5] })
}

const NAMES_DOW: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];
const NAMES_MONTH: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expr {
    minute: Vec<bool>,
    hour: Vec<bool>,
    day: Vec<bool>,
    month: Vec<bool>,
    dow: Vec<bool>,
    /// Vixie's rule: when both the day-of-month and the day-of-week are restricted, a match on *either* fires.
    day_restricted: bool,
    dow_restricted: bool,
}

fn field(spec: &str, lo: i64, hi: i64, names: &[&str]) -> Result<(Vec<bool>, bool)> {
    let mut set = vec![false; (hi - lo + 1) as usize];
    let mut restricted = true;
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            bail!("empty field");
        }
        let (range, step) = match part.split_once('/') {
            Some((r, s)) => (
                r,
                s.parse::<i64>().map_err(|_| anyhow::anyhow!("`{s}` is not a step"))?,
            ),
            None => (part, 1),
        };
        if step < 1 {
            bail!("a step must be 1 or more");
        }
        let one = |t: &str| -> Result<i64> {
            let t = t.trim().to_ascii_lowercase();
            if let Some(i) = names.iter().position(|n| *n == t) {
                return Ok(lo + i as i64);
            }
            t.parse::<i64>()
                .map_err(|_| anyhow::anyhow!("`{t}` is not a number in {lo}-{hi}"))
        };
        let (a, b) = if range == "*" {
            restricted = false;
            (lo, hi)
        } else if let Some((s, e)) = range.split_once('-') {
            (one(s)?, one(e)?)
        } else {
            let v = one(range)?;
            (v, v)
        };
        if a < lo || b > hi || a > b {
            bail!("{a}-{b} is outside {lo}-{hi}");
        }
        let mut v = a;
        while v <= b {
            set[(v - lo) as usize] = true;
            v += step;
        }
        if range == "*" && step > 1 {
            restricted = true;
        }
    }
    Ok((set, restricted))
}

impl Expr {
    pub fn parse(spec: &str) -> Result<Expr> {
        let spec = spec.trim();
        let expanded = match spec.to_ascii_lowercase().as_str() {
            "@yearly" | "@annually" => "0 0 1 1 *",
            "@monthly" => "0 0 1 * *",
            "@weekly" => "0 0 * * 0",
            "@daily" | "@midnight" => "0 0 * * *",
            "@hourly" => "0 * * * *",
            _ => spec,
        };
        let f: Vec<&str> = expanded.split_whitespace().collect();
        if f.len() != 5 {
            bail!(
                "a schedule is five fields - minute hour day month weekday - and this has {}: `{spec}`",
                f.len()
            );
        }
        let (minute, _) = field(f[0], 0, 59, &[]).context("minute")?;
        let (hour, _) = field(f[1], 0, 23, &[]).context("hour")?;
        let (day, day_restricted) = field(f[2], 1, 31, &[]).context("day of month")?;
        let (month, _) = field(f[3], 1, 12, &NAMES_MONTH).context("month")?;
        let dow_spec = f[4].replace('7', "0");
        let (dow, dow_restricted) = field(&dow_spec, 0, 6, &NAMES_DOW).context("weekday")?;
        Ok(Expr { minute, hour, day, month, dow, day_restricted, dow_restricted })
    }

    pub fn matches(&self, w: &When) -> bool {
        if !self.minute[w.minute as usize]
            || !self.hour[w.hour as usize]
            || !self.month[(w.month - 1) as usize]
        {
            return false;
        }
        let d = self.day[(w.day - 1) as usize];
        let wd = self.dow[w.dow as usize];
        if self.day_restricted && self.dow_restricted {
            d || wd
        } else {
            d && wd
        }
    }

    /// The next `n` firings after `from`, for a listing that answers "when does this actually run" rather than making someone read the fields.
    pub fn next(&self, from: &When, n: usize) -> Vec<When> {
        let mut out = Vec::new();
        let mut days = from.days();
        let mut hour = from.hour;
        let mut minute = from.minute;
        for _ in 0..(366 * 24 * 60) {
            minute += 1;
            if minute > 59 {
                minute = 0;
                hour += 1;
            }
            if hour > 23 {
                hour = 0;
                days += 1;
            }
            let w = When::from_days(days, hour, minute);
            if !self.month[(w.month - 1) as usize] {
                let (y, m, _) = civil_from_days(days);
                let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
                days = days_from_civil(ny, nm, 1);
                hour = 0;
                minute = -1;
                continue;
            }
            let dm = self.day[(w.day - 1) as usize];
            let dw = self.dow[w.dow as usize];
            let day_ok = if self.day_restricted && self.dow_restricted { dm || dw } else { dm && dw };
            if !day_ok {
                days += 1;
                hour = 0;
                minute = -1;
                continue;
            }
            if !self.hour[hour as usize] {
                hour += 1;
                minute = -1;
                if hour > 23 {
                    hour = 0;
                    days += 1;
                }
                continue;
            }
            if self.minute[minute as usize] {
                out.push(w);
                if out.len() == n {
                    break;
                }
            }
        }
        out
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub schedule: String,
    /// The repo the job runs in.
    pub repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pipeline: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// `run` drives the pipeline in the repo as it stands; `new` cuts a branch and stacks the work, the way a person would from Slack.
    #[serde(default = "default_kind")]
    pub kind: String,
    /// `name=value` for the pipeline's placeholders, as `--var` takes them.
    #[serde(default)]
    pub vars: Vec<String>,
    #[serde(default)]
    pub paused: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

fn default_kind() -> String {
    "run".into()
}

impl Job {
    /// What the job would have been typed as.
    pub fn argv(&self) -> Vec<String> {
        let mut v = Vec::new();
        match self.kind.as_str() {
            "new" => {
                v.push("new".into());
                v.push(self.id.clone());
                if let Some(t) = &self.task {
                    v.push(t.clone());
                }
                if let Some(p) = &self.pipeline {
                    v.push("--pipeline".into());
                    v.push(p.clone());
                }
                v.push("--fg".into());
            }
            _ => {
                v.push("run".into());
                if let Some(p) = &self.pipeline {
                    v.push("--pipeline".into());
                    v.push(p.clone());
                }
                if let Some(t) = &self.task {
                    v.push("--task".into());
                    v.push(t.clone());
                }
            }
        }
        for kv in &self.vars {
            v.push("--var".into());
            v.push(kv.clone());
        }
        v
    }

    pub fn describe(&self) -> String {
        let mut s = String::new();
        if let Some(p) = &self.pipeline {
            s.push_str(p);
        }
        if let Some(t) = &self.task {
            if !s.is_empty() {
                s.push_str(" · ");
            }
            s.push_str(t);
        }
        if s.is_empty() {
            s.push_str("(nothing to do)");
        }
        s
    }
}

pub fn load() -> Result<Vec<Job>> {
    let path = instance::cron_path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs_read(&path)?;
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&text).with_context(|| format!("reading {}", path.display()))
}

pub fn save(jobs: &[Job]) -> Result<()> {
    let path = instance::cron_path();
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(jobs)? + "\n")?;
    std::fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn fs_read(p: &Path) -> Result<String> {
    std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))
}

/// Resolve any unambiguous part of an id, the way every other rigg verb does with a stack name.
pub fn find<'a>(jobs: &'a [Job], want: &str) -> Result<&'a Job> {
    if let Some(j) = jobs.iter().find(|j| j.id == want) {
        return Ok(j);
    }
    let hits: Vec<&Job> = jobs.iter().filter(|j| j.id.contains(want)).collect();
    match hits.len() {
        0 => bail!("no job matching `{want}` - `rigg cron` lists them"),
        1 => Ok(hits[0]),
        _ => bail!(
            "`{want}` matches {}",
            hits.iter().map(|j| j.id.as_str()).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// Is the process that last ran this job still going? Checking the name as well as the pid because pids are reused, and a job skipped for the life of some unrelated process is a job that silently stops running.
pub fn alive(pid: u32) -> bool {
    match std::fs::read(format!("/proc/{pid}/cmdline")) {
        Ok(b) => String::from_utf8_lossy(&b).contains("rigg"),
        Err(_) => false,
    }
}

/// Removes the lock file when the tick ends, however it ends.
pub struct TickLock(std::path::PathBuf);

impl Drop for TickLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// One tick per instance. Cron will knock again in a minute either way, so a
/// tick that finds another in flight says nothing and leaves.
fn hold_tick() -> Result<Option<TickLock>> {
    let path = instance::dir().join("tick.pid");
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(pid) = text.trim().parse::<u32>() {
            if alive(pid) {
                return Ok(None);
            }
        }
    }
    std::fs::write(&path, std::process::id().to_string())?;
    Ok(Some(TickLock(path)))
}

pub struct Fired {
    pub id: String,
    pub why: String,
}

/// One knock.
pub fn tick(at: Option<&str>, dry_run: bool) -> Result<Vec<Fired>> {
    let _lock = if dry_run {
        None
    } else {
        match hold_tick()? {
            Some(l) => Some(l),
            // Cron knocks again in a minute either way.
            None => {
                return Ok(vec![Fired {
                    id: "tick".into(),
                    why: "another tick is still going".into(),
                }])
            }
        }
    };
    let when = now(at)?;
    let mut jobs = load()?;
    let mut fired = Vec::new();
    let mut dirty = false;

    for i in 0..jobs.len() {
        let expr = match Expr::parse(&jobs[i].schedule) {
            Ok(e) => e,
            Err(e) => {
                if jobs[i].last_status.as_deref() != Some("bad-schedule") {
                    jobs[i].last_status = Some("bad-schedule".into());
                    jobs[i].last_detail = Some(format!("{e:#}"));
                    dirty = true;
                }
                continue;
            }
        };
        if jobs[i].paused || !expr.matches(&when) {
            continue;
        }
        if let Some(pid) = jobs[i].pid {
            if alive(pid) {
                fired.push(Fired {
                    id: jobs[i].id.clone(),
                    why: format!("skipped - still running from {}", jobs[i].last_run.as_deref().unwrap_or("earlier")),
                });
                continue;
            }
        }
        if !Path::new(&jobs[i].repo).is_dir() {
            if jobs[i].last_status.as_deref() != Some("no-repo") {
                jobs[i].last_status = Some("no-repo".into());
                jobs[i].last_detail = Some(format!("{} is not there", jobs[i].repo));
                dirty = true;
            }
            fired.push(Fired { id: jobs[i].id.clone(), why: format!("skipped - {} is not there", jobs[i].repo) });
            continue;
        }
        if dry_run {
            fired.push(Fired {
                id: jobs[i].id.clone(),
                why: format!("would run: rigg {}", jobs[i].argv().join(" ")),
            });
            continue;
        }
        let pid = spawn_runner(&jobs[i])?;
        jobs[i].pid = Some(pid);
        jobs[i].last_run = Some(when.stamp());
        jobs[i].last_status = Some("running".into());
        jobs[i].last_detail = None;
        dirty = true;
        fired.push(Fired { id: jobs[i].id.clone(), why: format!("started (pid {pid})") });
    }

    if dirty {
        save(&jobs)?;
    }
    Ok(fired)
}

/// Where a job's own output goes.
pub fn log_path(id: &str) -> std::path::PathBuf {
    instance::dir().join("logs").join(format!("{id}.log"))
}

/// Start `rigg cron run <id>` detached, so the tick returns in milliseconds and the job owns its own lifetime.
fn spawn_runner(job: &Job) -> Result<u32> {
    let exe = std::env::current_exe().context("finding the rigg binary")?;
    let log = log_path(&job.id);
    if let Some(p) = log.parent() {
        std::fs::create_dir_all(p)?;
    }
    let out = std::fs::File::create(&log)
        .with_context(|| format!("opening {}", log.display()))?;
    let err = out.try_clone()?;
    let child = Command::new("setsid")
        .arg(&exe)
        .arg("cron")
        .arg("run")
        .arg(&job.id)
        .arg("--fired")
        .current_dir(&job.repo)
        .env("PWD", &job.repo)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err))
        .spawn()
        .context("spawning the job under setsid")?;
    Ok(child.id())
}

/// Run one job to completion here, recording what happened.
pub fn run_job(id: &str, fired: bool) -> Result<()> {
    let jobs = load()?;
    let job = find(&jobs, id)?.clone();
    // setsid usually execs in place, so its pid is the job's - but it forks
    // when it is already a group leader, and then the pid the tick recorded
    // dies immediately and the overlap guard stops guarding. Record our own.
    {
        let mut jobs = load()?;
        if let Some(j) = jobs.iter_mut().find(|j| j.id == job.id) {
            j.pid = Some(std::process::id());
        }
        save(&jobs)?;
    }
    let exe = std::env::current_exe().context("finding the rigg binary")?;
    let argv = job.argv();
    println!("rigg {}", argv.join(" "));
    let status = Command::new(&exe)
        .args(&argv)
        .current_dir(&job.repo)
        .env("PWD", &job.repo)
        .stdin(Stdio::null())
        .status()
        .with_context(|| format!("running job `{}`", job.id))?;

    let mut jobs = load()?;
    if let Some(j) = jobs.iter_mut().find(|j| j.id == job.id) {
        j.pid = None;
        j.last_status = Some(if status.success() { "ok".into() } else { "failed".into() });
        j.last_detail = if status.success() {
            None
        } else {
            Some(format!("exit {}", status.code().unwrap_or(-1)))
        };
        if !fired {
            j.last_run = Some(now(None)?.stamp());
        }
    }
    save(&jobs)?;
    if !status.success() {
        bail!("job `{}` exited with status {}", job.id, status.code().unwrap_or(-1));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(y: i64, m: i64, d: i64, h: i64, mi: i64) -> When {
        When { year: y, month: m, day: d, hour: h, minute: mi, dow: weekday(days_from_civil(y, m, d)) }
    }

    #[test]
    fn a_plain_schedule_matches_its_minute() {
        let e = Expr::parse("0 7 * * *").unwrap();
        assert!(e.matches(&w(2026, 9, 9, 7, 0)));
        assert!(!e.matches(&w(2026, 9, 9, 7, 1)));
        assert!(!e.matches(&w(2026, 9, 9, 8, 0)));
    }

    #[test]
    fn a_step_reaches_every_nth_minute() {
        let e = Expr::parse("*/15 * * * *").unwrap();
        for m in [0, 15, 30, 45] {
            assert!(e.matches(&w(2026, 9, 9, 3, m)), "{m}");
        }
        assert!(!e.matches(&w(2026, 9, 9, 3, 14)));
    }

    #[test]
    fn day_and_weekday_are_an_or_when_both_are_named() {
        let e = Expr::parse("0 0 1 * mon").unwrap();
        assert!(e.matches(&w(2026, 9, 1, 0, 0)), "the first");
        assert!(e.matches(&w(2026, 9, 7, 0, 0)), "a monday");
        assert!(!e.matches(&w(2026, 9, 8, 0, 0)), "a tuesday that is not the first");
    }

    #[test]
    fn day_and_weekday_are_an_and_when_only_one_is() {
        let e = Expr::parse("0 0 * * mon").unwrap();
        assert!(e.matches(&w(2026, 9, 7, 0, 0)));
        assert!(!e.matches(&w(2026, 9, 8, 0, 0)));
    }

    #[test]
    fn sunday_is_both_nought_and_seven() {
        let a = Expr::parse("0 0 * * 0").unwrap();
        let b = Expr::parse("0 0 * * 7").unwrap();
        assert_eq!(a, b);
        assert!(a.matches(&w(2026, 9, 6, 0, 0)));
    }

    #[test]
    fn an_alias_is_the_schedule_it_stands_for() {
        assert_eq!(Expr::parse("@daily").unwrap(), Expr::parse("0 0 * * *").unwrap());
        assert_eq!(Expr::parse("@hourly").unwrap(), Expr::parse("0 * * * *").unwrap());
    }

    #[test]
    fn a_schedule_that_is_not_five_fields_says_so() {
        let e = Expr::parse("0 7 * *").unwrap_err().to_string();
        assert!(e.contains("five fields"), "{e}");
    }

    #[test]
    fn the_next_firings_are_the_ones_a_reader_would_predict() {
        let e = Expr::parse("30 6 * * *").unwrap();
        let n = e.next(&w(2026, 9, 9, 7, 0), 2);
        assert_eq!(n[0].stamp(), "2026-09-10 06:30");
        assert_eq!(n[1].stamp(), "2026-09-11 06:30");
    }

    #[test]
    fn a_yearly_schedule_is_still_cheap_to_describe() {
        let e = Expr::parse("@yearly").unwrap();
        let n = e.next(&w(2026, 9, 9, 7, 0), 1);
        assert_eq!(n[0].stamp(), "2027-01-01 00:00");
    }

    #[test]
    fn a_month_boundary_does_not_lose_a_day() {
        let e = Expr::parse("0 0 1 * *").unwrap();
        let n = e.next(&w(2026, 12, 15, 0, 0), 2);
        assert_eq!(n[0].stamp(), "2027-01-01 00:00");
        assert_eq!(n[1].stamp(), "2027-02-01 00:00");
    }

    #[test]
    fn a_run_job_is_the_argv_a_person_would_have_typed() {
        let j = Job {
            id: "corpus".into(),
            schedule: "*/15 * * * *".into(),
            repo: "/tmp".into(),
            pipeline: Some("corpus-sync".into()),
            task: None,
            kind: "run".into(),
            vars: Vec::new(),
            paused: false,
            created: None,
            last_run: None,
            last_status: None,
            last_detail: None,
            pid: None,
        };
        assert_eq!(j.argv(), vec!["run", "--pipeline", "corpus-sync"]);
    }
}
