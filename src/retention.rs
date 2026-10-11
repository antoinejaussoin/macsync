use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDateTime};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Snapshot names are local times like `2026-10-11T081300`.
pub const STAMP_FORMAT: &str = "%Y-%m-%dT%H%M%S";

pub fn parse_stamp(name: &str) -> Option<NaiveDateTime> {
    if name.len() != 17 {
        return None;
    }
    NaiveDateTime::parse_from_str(name, STAMP_FORMAT).ok()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Retention {
    pub keep_last: u32,
    pub keep_daily: u32,
    pub keep_weekly: u32,
    pub keep_monthly: u32,
    pub keep_yearly: KeepYearly,
}

impl Default for Retention {
    fn default() -> Self {
        Retention {
            keep_last: 30,
            keep_daily: 0,
            keep_weekly: 0,
            keep_monthly: 3,
            keep_yearly: KeepYearly::Forever,
        }
    }
}

/// Stored as `"forever"` or a number of years (0 turns the tier off).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepYearly {
    Forever,
    Count(u32),
}

impl KeepYearly {
    fn limit(self) -> usize {
        match self {
            KeepYearly::Forever => usize::MAX,
            KeepYearly::Count(n) => n as usize,
        }
    }
}

impl Serialize for KeepYearly {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            KeepYearly::Forever => s.serialize_str("forever"),
            KeepYearly::Count(n) => s.serialize_u32(*n),
        }
    }
}

impl<'de> Deserialize<'de> for KeepYearly {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Count(u32),
            Word(String),
        }
        match Raw::deserialize(d)? {
            Raw::Count(n) => Ok(KeepYearly::Count(n)),
            Raw::Word(w) if w.eq_ignore_ascii_case("forever") => Ok(KeepYearly::Forever),
            Raw::Word(w) => Err(serde::de::Error::custom(format!(
                "keep_yearly must be \"forever\" or a number, got \"{w}\""
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Latest,
    Last,
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self {
            Tier::Latest => "latest",
            Tier::Last => "last",
            Tier::Daily => "daily",
            Tier::Weekly => "weekly",
            Tier::Monthly => "monthly",
            Tier::Yearly => "yearly",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub name: String,
    pub time: NaiveDateTime,
    /// Tiers that keep this snapshot; empty means it will be removed.
    pub kept_by: Vec<Tier>,
}

impl Decision {
    pub fn keep(&self) -> bool {
        !self.kept_by.is_empty()
    }
}

/// Decides which snapshots survive, newest first.
///
/// Each calendar tier walks the snapshots newest to oldest and keeps the newest
/// snapshot of each of its N most recent buckets (day, ISO week, month, year).
/// Buckets are counted even when their snapshot is already kept by another tier,
/// so "3 months" means the 3 newest months, not 3 extra snapshots. The snapshot
/// `latest` points at is always kept.
pub fn plan(snapshots: &[(String, NaiveDateTime)], policy: &Retention, latest: Option<&str>) -> Vec<Decision> {
    let mut sorted: Vec<&(String, NaiveDateTime)> = snapshots.iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(&a.0)));

    let mut kept: BTreeMap<usize, Vec<Tier>> = BTreeMap::new();
    let mut keep = |i: usize, tier: Tier| kept.entry(i).or_default().push(tier);

    for (i, (name, _)) in sorted.iter().enumerate() {
        if latest == Some(name.as_str()) {
            keep(i, Tier::Latest);
        }
    }
    for i in 0..sorted.len().min(policy.keep_last as usize) {
        keep(i, Tier::Last);
    }

    type Bucket = fn(&NaiveDateTime) -> (i32, u32, u32);
    let tiers: [(Tier, usize, Bucket); 4] = [
        (Tier::Daily, policy.keep_daily as usize, |t| {
            (t.year(), t.month(), t.day())
        }),
        (Tier::Weekly, policy.keep_weekly as usize, |t| {
            let w = t.iso_week();
            (w.year(), w.week(), 0)
        }),
        (Tier::Monthly, policy.keep_monthly as usize, |t| {
            (t.year(), t.month(), 0)
        }),
        (Tier::Yearly, policy.keep_yearly.limit(), |t| (t.year(), 0, 0)),
    ];
    for (tier, limit, bucket) in tiers {
        let mut last_bucket = None;
        let mut buckets_seen = 0usize;
        for (i, (_, time)) in sorted.iter().enumerate() {
            if buckets_seen >= limit {
                break;
            }
            let b = bucket(time);
            if last_bucket != Some(b) {
                last_bucket = Some(b);
                buckets_seen += 1;
                keep(i, tier);
            }
        }
    }

    sorted
        .iter()
        .enumerate()
        .map(|(i, (name, time))| Decision {
            name: name.clone(),
            time: *time,
            kept_by: kept.remove(&i).unwrap_or_default(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, NaiveDate};

    fn daily_history(from: NaiveDate, to: NaiveDate) -> Vec<(String, NaiveDateTime)> {
        let mut out = Vec::new();
        let mut day = from;
        while day <= to {
            let t = day.and_hms_opt(2, 0, 0).unwrap();
            out.push((t.format(STAMP_FORMAT).to_string(), t));
            day += Duration::days(1);
        }
        out
    }

    fn kept_names(decisions: &[Decision]) -> Vec<String> {
        decisions.iter().filter(|d| d.keep()).map(|d| d.name.clone()).collect()
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn stamps() {
        assert!(parse_stamp("2026-10-11T081300").is_some());
        assert!(parse_stamp("2026-10-11T08130").is_none());
        assert!(parse_stamp(".inprogress-2026-10-11T081300").is_none());
        assert!(parse_stamp("latest").is_none());
        assert!(parse_stamp("2026-13-11T081300").is_none());
    }

    #[test]
    fn default_policy_on_daily_history() {
        let snaps = daily_history(date(2024, 3, 1), date(2026, 10, 11));
        let decisions = plan(&snaps, &Retention::default(), Some("2026-10-11T020000"));
        let kept = kept_names(&decisions);

        let mut expected: Vec<String> = (12..=30).rev().map(|d| format!("2026-09-{d:02}T020000")).collect();
        expected.splice(0..0, (1..=11).rev().map(|d| format!("2026-10-{d:02}T020000")));
        expected.push("2026-08-31T020000".into());
        expected.push("2025-12-31T020000".into());
        expected.push("2024-12-31T020000".into());
        assert_eq!(kept, expected);

        let newest = &decisions[0];
        assert_eq!(
            newest.kept_by,
            vec![Tier::Latest, Tier::Last, Tier::Monthly, Tier::Yearly]
        );
        let august = decisions.iter().find(|d| d.name == "2026-08-31T020000").unwrap();
        assert_eq!(august.kept_by, vec![Tier::Monthly]);
    }

    #[test]
    fn month_covered_by_last_is_not_duplicated() {
        // 40 daily snapshots, Oct 1 to Nov 9. Monthly 2 picks Nov 9 and Oct 31,
        // both already inside the last 30, so nothing extra is kept.
        let snaps = daily_history(date(2026, 10, 1), date(2026, 11, 9));
        let policy = Retention {
            keep_last: 30,
            keep_daily: 0,
            keep_weekly: 0,
            keep_monthly: 2,
            keep_yearly: KeepYearly::Count(0),
        };
        let kept = kept_names(&plan(&snaps, &policy, None));
        assert_eq!(kept.len(), 30);
        assert!(kept.contains(&"2026-10-31T020000".to_string()));
    }

    #[test]
    fn yearly_forever_vs_cap() {
        let snaps = daily_history(date(2020, 1, 1), date(2026, 10, 11));
        let only_yearly = |keep_yearly| Retention {
            keep_last: 1,
            keep_daily: 0,
            keep_weekly: 0,
            keep_monthly: 0,
            keep_yearly,
        };
        let forever = kept_names(&plan(&snaps, &only_yearly(KeepYearly::Forever), None));
        assert_eq!(forever.len(), 7, "{forever:?}");
        assert!(forever.contains(&"2020-12-31T020000".to_string()));

        let capped = kept_names(&plan(&snaps, &only_yearly(KeepYearly::Count(3)), None));
        assert_eq!(capped, ["2026-10-11T020000", "2025-12-31T020000", "2024-12-31T020000"]);
    }

    #[test]
    fn zero_tiers_are_off_but_latest_survives() {
        let snaps = daily_history(date(2026, 1, 1), date(2026, 3, 1));
        let none = Retention {
            keep_last: 0,
            keep_daily: 0,
            keep_weekly: 0,
            keep_monthly: 0,
            keep_yearly: KeepYearly::Count(0),
        };
        let decisions = plan(&snaps, &none, Some("2026-02-01T020000"));
        assert_eq!(kept_names(&decisions), ["2026-02-01T020000"]);
        assert_eq!(decisions.iter().find(|d| d.keep()).unwrap().kept_by, vec![Tier::Latest]);
    }

    #[test]
    fn daily_and_weekly_pick_newest_in_bucket() {
        // Hourly snapshots over 3 days.
        let mut snaps = Vec::new();
        for day in 1..=3 {
            for hour in 0..24 {
                let t = date(2026, 6, day).and_hms_opt(hour, 0, 0).unwrap();
                snaps.push((t.format(STAMP_FORMAT).to_string(), t));
            }
        }
        let policy = Retention {
            keep_last: 2,
            keep_daily: 3,
            keep_weekly: 1,
            keep_monthly: 0,
            keep_yearly: KeepYearly::Count(0),
        };
        let kept = kept_names(&plan(&snaps, &policy, None));
        assert_eq!(
            kept,
            [
                "2026-06-03T230000",
                "2026-06-03T220000",
                "2026-06-02T230000",
                "2026-06-01T230000"
            ]
        );
    }
}
