use chrono::{DateTime, Local};
use croner::Cron;
use croner::parser::{CronParser, Seconds, Year};

/// Standard 5-field cron (minute hour day-of-month month day-of-week), Sunday = 0.
pub fn parse(expr: &str) -> Result<Cron, String> {
    CronParser::builder()
        .seconds(Seconds::Disallowed)
        .year(Year::Disallowed)
        .build()
        .parse(expr.trim())
        .map_err(|e| format!("Invalid schedule \"{}\": {e}", expr.trim()))
}

/// Empty means manual only.
pub fn validate(expr: &str) -> Result<(), String> {
    if expr.trim().is_empty() {
        return Ok(());
    }
    parse(expr).map(|_| ())
}

pub fn next_after(expr: &str, after: DateTime<Local>) -> Option<DateTime<Local>> {
    if expr.trim().is_empty() {
        return None;
    }
    parse(expr).ok()?.find_next_occurrence(&after, false).ok()
}

/// A job is due when a fire time has passed since the anchor (the last attempt,
/// or when the agent first saw the pair). After sleep this yields a single
/// catch-up run, because the anchor moves to the time of that run.
pub fn is_due(expr: &str, anchor: DateTime<Local>, now: DateTime<Local>) -> bool {
    next_after(expr, anchor).is_some_and(|next| next <= now)
}

pub const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Manual,
    Hourly { minute: u32 },
    Daily { hour: u32, minute: u32 },
    Weekly { weekday: u32, hour: u32, minute: u32 },
    Custom,
}

impl Preset {
    pub fn label(&self) -> &'static str {
        match self {
            Preset::Manual => "Manual only",
            Preset::Hourly { .. } => "Every hour",
            Preset::Daily { .. } => "Every day",
            Preset::Weekly { .. } => "Every week",
            Preset::Custom => "Custom (cron)",
        }
    }

    pub fn to_cron(self) -> Option<String> {
        match self {
            Preset::Manual => Some(String::new()),
            Preset::Hourly { minute } => Some(format!("{minute} * * * *")),
            Preset::Daily { hour, minute } => Some(format!("{minute} {hour} * * *")),
            Preset::Weekly { weekday, hour, minute } => Some(format!("{minute} {hour} * * {weekday}")),
            Preset::Custom => None,
        }
    }

    pub fn from_cron(expr: &str) -> Preset {
        let fields: Vec<&str> = expr.split_whitespace().collect();
        if fields.is_empty() {
            return Preset::Manual;
        }
        if fields.len() != 5 {
            return Preset::Custom;
        }
        let num = |s: &str, max: u32| s.parse::<u32>().ok().filter(|v| *v <= max);
        let (minute, hour) = (num(fields[0], 59), num(fields[1], 23));
        match (minute, fields[1], hour, fields[2], fields[3], fields[4]) {
            (Some(minute), "*", _, "*", "*", "*") => Preset::Hourly { minute },
            (Some(minute), _, Some(hour), "*", "*", "*") => Preset::Daily { hour, minute },
            (Some(minute), _, Some(hour), "*", "*", dow) => match num(dow, 7) {
                Some(weekday) => Preset::Weekly {
                    weekday: weekday % 7,
                    hour,
                    minute,
                },
                None => Preset::Custom,
            },
            _ => Preset::Custom,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn next_run_uses_standard_cron() {
        let next = next_after("30 2 * * *", at(2026, 10, 11, 8, 0)).unwrap();
        assert_eq!(next, at(2026, 10, 12, 2, 30));
        // Sunday is 0, like system cron. 2026-10-11 is a Sunday.
        let next = next_after("0 9 * * 0", at(2026, 10, 10, 12, 0)).unwrap();
        assert_eq!(next, at(2026, 10, 11, 9, 0));
        assert!(next_after("", at(2026, 10, 11, 8, 0)).is_none());
        assert!(validate("0 0 0 * * *").is_err(), "seconds field is not accepted");
        assert!(validate("nope").is_err());
    }

    #[test]
    fn catch_up_runs_once_after_sleep() {
        let last_run = at(2026, 10, 8, 2, 0);
        let woke = at(2026, 10, 11, 9, 0);
        assert!(is_due("0 2 * * *", last_run, woke));
        // The catch-up run becomes the new anchor: nothing more until tomorrow 02:00.
        let caught_up = at(2026, 10, 11, 9, 0);
        assert!(!is_due("0 2 * * *", caught_up, at(2026, 10, 11, 23, 59)));
        assert!(is_due("0 2 * * *", caught_up, at(2026, 10, 12, 2, 0)));
    }

    #[test]
    fn presets_round_trip() {
        for preset in [
            Preset::Manual,
            Preset::Hourly { minute: 15 },
            Preset::Daily { hour: 2, minute: 0 },
            Preset::Weekly {
                weekday: 1,
                hour: 3,
                minute: 30,
            },
        ] {
            let cron = preset.to_cron().unwrap();
            assert_eq!(Preset::from_cron(&cron), preset, "{cron}");
            validate(&cron).unwrap();
        }
        assert_eq!(Preset::from_cron("*/15 9-17 * * 1-5"), Preset::Custom);
    }
}
