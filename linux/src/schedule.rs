use chrono::{Datelike, Local, NaiveDateTime, Timelike, Weekday};

/// 5-field cron: minute hour day-of-month month weekday.
/// Weekday: Sunday = 0 (also accepts 7 for Sunday). Empty string = disabled.
#[derive(Debug, Clone, PartialEq)]
pub struct CronExpr {
    minutes: Field,
    hours: Field,
    days: Field,
    months: Field,
    weekdays: Field,
}

#[derive(Debug, Clone, PartialEq)]
enum Field {
    Any,
    Values(Vec<u32>),
}

impl CronExpr {
    pub fn parse(expr: &str) -> Result<Option<Self>, String> {
        let expr = expr.trim();
        if expr.is_empty() {
            return Ok(None);
        }
        let parts: Vec<&str> = expr.split_whitespace().collect();
        if parts.len() != 5 {
            return Err(format!(
                "cron must have 5 fields (got {}): '{expr}'",
                parts.len()
            ));
        }
        Ok(Some(Self {
            minutes: Field::parse(parts[0], 0, 59)?,
            hours: Field::parse(parts[1], 0, 23)?,
            days: Field::parse(parts[2], 1, 31)?,
            months: Field::parse(parts[3], 1, 12)?,
            weekdays: Field::parse_weekday(parts[4])?,
        }))
    }

    pub fn matches(&self, dt: NaiveDateTime) -> bool {
        let minute = dt.minute();
        let hour = dt.hour();
        let day = dt.day();
        let month = dt.month();
        let weekday = match dt.weekday() {
            Weekday::Sun => 0,
            Weekday::Mon => 1,
            Weekday::Tue => 2,
            Weekday::Wed => 3,
            Weekday::Thu => 4,
            Weekday::Fri => 5,
            Weekday::Sat => 6,
        };
        self.minutes.contains(minute)
            && self.hours.contains(hour)
            && self.months.contains(month)
            && self.days_and_weekdays(day, weekday)
    }

    fn days_and_weekdays(&self, day: u32, weekday: u32) -> bool {
        // Standard cron: if both day-of-month and weekday are restricted, OR them.
        let day_any = matches!(self.days, Field::Any);
        let wd_any = matches!(self.weekdays, Field::Any);
        match (day_any, wd_any) {
            (true, true) => true,
            (true, false) => self.weekdays.contains(weekday),
            (false, true) => self.days.contains(day),
            (false, false) => self.days.contains(day) || self.weekdays.contains(weekday),
        }
    }
}

impl Field {
    fn parse(s: &str, min: u32, max: u32) -> Result<Self, String> {
        if s == "*" {
            return Ok(Field::Any);
        }
        let mut values = Vec::new();
        for part in s.split(',') {
            if let Some((range, step_s)) = part.split_once('/') {
                let step: u32 = step_s
                    .parse()
                    .map_err(|_| format!("invalid step in '{part}'"))?;
                if step == 0 {
                    return Err(format!("step cannot be 0 in '{part}'"));
                }
                let (lo, hi) = if range == "*" {
                    (min, max)
                } else if let Some((a, b)) = range.split_once('-') {
                    (
                        a.parse().map_err(|_| format!("bad range '{part}'"))?,
                        b.parse().map_err(|_| format!("bad range '{part}'"))?,
                    )
                } else {
                    let v: u32 = range.parse().map_err(|_| format!("bad field '{part}'"))?;
                    (v, max)
                };
                let mut v = lo;
                while v <= hi {
                    if v >= min && v <= max {
                        values.push(v);
                    }
                    v += step;
                }
            } else if let Some((a, b)) = part.split_once('-') {
                let lo: u32 = a.parse().map_err(|_| format!("bad range '{part}'"))?;
                let hi: u32 = b.parse().map_err(|_| format!("bad range '{part}'"))?;
                for v in lo..=hi {
                    if v >= min && v <= max {
                        values.push(v);
                    }
                }
            } else {
                let v: u32 = part.parse().map_err(|_| format!("bad field '{part}'"))?;
                if v < min || v > max {
                    return Err(format!("value {v} out of range {min}-{max}"));
                }
                values.push(v);
            }
        }
        values.sort_unstable();
        values.dedup();
        Ok(Field::Values(values))
    }

    fn parse_weekday(s: &str) -> Result<Self, String> {
        // Allow 0-7 where 7 means Sunday.
        let field = Field::parse(s, 0, 7)?;
        Ok(match field {
            Field::Any => Field::Any,
            Field::Values(vs) => {
                let mut mapped: Vec<u32> = vs
                    .into_iter()
                    .map(|v| if v == 7 { 0 } else { v })
                    .collect();
                mapped.sort_unstable();
                mapped.dedup();
                Field::Values(mapped)
            }
        })
    }

    fn contains(&self, v: u32) -> bool {
        match self {
            Field::Any => true,
            Field::Values(vs) => vs.contains(&v),
        }
    }
}

/// Tick every 15 seconds: if wall clock minute changed into a matching slot since last_check,
/// fire once. Catch-up: if machine was down, fire at most one missed run when coming back.
#[derive(Debug, Clone)]
pub struct ScheduleTracker {
    pub last_tick: NaiveDateTime,
}

impl ScheduleTracker {
    pub fn new(now: NaiveDateTime) -> Self {
        Self { last_tick: now }
    }

    /// Returns true if `cron` should fire between `last_tick` (exclusive) and `now` (inclusive),
    /// truncated to minute resolution, with at most one catch-up.
    #[cfg(test)]
    pub fn should_fire(&mut self, cron: &CronExpr, now: NaiveDateTime) -> bool {
        let start = truncate_minute(self.last_tick);
        let end = truncate_minute(now);
        self.last_tick = now;

        if end <= start {
            // Same minute: only fire if we just entered this minute from a previous tick
            // that was already in this minute — skip duplicates within the minute.
            return false;
        }

        // Walk minutes from start+1min through end; fire on first match (catch-up = one run).
        let mut t = start + chrono::Duration::minutes(1);
        while t <= end {
            if cron.matches(t) {
                return true;
            }
            t += chrono::Duration::minutes(1);
        }
        false
    }
}

pub fn truncate_minute(dt: NaiveDateTime) -> NaiveDateTime {
    dt.with_second(0)
        .unwrap()
        .with_nanosecond(0)
        .unwrap()
}

pub fn now_local() -> NaiveDateTime {
    Local::now().naive_local()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn dt(y: i32, m: u32, d: u32, h: u32, min: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, min, 0)
            .unwrap()
    }

    #[test]
    fn parse_empty() {
        assert!(CronExpr::parse("").unwrap().is_none());
        assert!(CronExpr::parse("   ").unwrap().is_none());
    }

    #[test]
    fn daily_2am() {
        let c = CronExpr::parse("0 2 * * *").unwrap().unwrap();
        assert!(c.matches(dt(2024, 1, 1, 2, 0)));
        assert!(!c.matches(dt(2024, 1, 1, 2, 1)));
        assert!(!c.matches(dt(2024, 1, 1, 3, 0)));
    }

    #[test]
    fn sunday_morning() {
        // 2024-01-07 is Sunday
        let c = CronExpr::parse("0 4 * * 0").unwrap().unwrap();
        assert!(c.matches(dt(2024, 1, 7, 4, 0)));
        assert!(!c.matches(dt(2024, 1, 8, 4, 0))); // Monday
        let c7 = CronExpr::parse("0 4 * * 7").unwrap().unwrap();
        assert!(c7.matches(dt(2024, 1, 7, 4, 0)));
    }

    #[test]
    fn catch_up_once() {
        let c = CronExpr::parse("0 2 * * *").unwrap().unwrap();
        let mut tracker = ScheduleTracker::new(dt(2024, 1, 1, 1, 0));
        // Jump past 02:00 to 05:00 — should fire once
        assert!(tracker.should_fire(&c, dt(2024, 1, 1, 5, 0)));
        // Same window again with last_tick updated — no second fire without new slot
        assert!(!tracker.should_fire(&c, dt(2024, 1, 1, 5, 0)));
        // Next day 2am
        assert!(tracker.should_fire(&c, dt(2024, 1, 2, 2, 0)));
    }

    #[test]
    fn step_minutes() {
        let c = CronExpr::parse("*/15 * * * *").unwrap().unwrap();
        assert!(c.matches(dt(2024, 1, 1, 0, 0)));
        assert!(c.matches(dt(2024, 1, 1, 0, 15)));
        assert!(!c.matches(dt(2024, 1, 1, 0, 7)));
    }
}
