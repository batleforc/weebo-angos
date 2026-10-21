//! A 5-field cron expression (minute hour day-of-month month day-of-week),
//! in UTC: `*`, `a`, `a-b`, `*/n`, `a-b/n` and lists of them; day-of-week
//! 0-7 from Sunday (both 0 and 7 are Sunday). When both day fields are
//! restricted, a day matching either runs (Vixie cron).

use k8s_openapi::jiff::civil::{Date, DateTime, Time};
use k8s_openapi::jiff::tz::TimeZone;
use k8s_openapi::jiff::{Timestamp, ToSpan};

/// Why a schedule doesn't read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ScheduleError(pub String);

/// Values `0..=63` of a field, one bit each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Set(u64);

impl Set {
    fn has(self, n: i8) -> bool {
        u32::try_from(n)
            .ok()
            .and_then(|n| self.0.checked_shr(n))
            .is_some_and(|bits| bits & 1 == 1)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    minutes: Set,
    hours: Set,
    days: Set,
    months: Set,
    weekdays: Set,
    /// Day-of-month written `*`.
    any_day: bool,
    /// Day-of-week written `*`.
    any_weekday: bool,
}

/// Parses one field, values in `min..=max`.
fn field(text: &str, min: u32, max: u32, name: &str) -> Result<Set, String> {
    let mut set = 0u64;
    for part in text.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => {
                let step: u32 = step
                    .parse()
                    .map_err(|_| format!("{name}: step {step:?} is not a number"))?;
                if step == 0 {
                    return Err(format!("{name}: a step of 0"));
                }
                (range, step)
            }
            None => (part, 1),
        };
        let number = |n: &str| -> Result<u32, String> {
            let n: u32 = n
                .parse()
                .map_err(|_| format!("{name}: {n:?} is not a number"))?;
            if n < min || n > max {
                return Err(format!("{name}: {n} is outside {min}-{max}"));
            }
            Ok(n)
        };
        let (from, to) = match range {
            "*" => (min, max),
            range => match range.split_once('-') {
                Some((a, b)) => (number(a)?, number(b)?),
                None if step > 1 => (number(range)?, max),
                None => {
                    let n = number(range)?;
                    (n, n)
                }
            },
        };
        if from > to {
            return Err(format!("{name}: {from}-{to} is empty"));
        }
        for n in (from..=to).step_by(step as usize) {
            set |= 1u64.checked_shl(n).unwrap_or(0);
        }
    }
    Ok(Set(set))
}

impl Schedule {
    pub fn parse(text: &str) -> Result<Self, ScheduleError> {
        Self::read(text).map_err(ScheduleError)
    }

    fn read(text: &str) -> Result<Self, String> {
        let fields: Vec<&str> = text.split_whitespace().collect();
        let [minute, hour, day, month, weekday] = fields[..] else {
            return Err(format!(
                "{text:?}: 5 fields expected (minute hour day-of-month month day-of-week), {} found",
                fields.len()
            ));
        };
        let weekdays = field(weekday, 0, 7, "day-of-week")?;
        // 7 is Sunday too.
        let weekdays = Set((weekdays.0 | u64::from(weekdays.has(7))) & 0x7f);
        Ok(Self {
            minutes: field(minute, 0, 59, "minute")?,
            hours: field(hour, 0, 23, "hour")?,
            days: field(day, 1, 31, "day-of-month")?,
            months: field(month, 1, 12, "month")?,
            weekdays,
            // Vixie cron: a field starting with `*` (`*/2` too) is
            // unrestricted for the day-of-month / day-of-week OR rule.
            any_day: day.starts_with('*'),
            any_weekday: weekday.starts_with('*'),
        })
    }

    fn day_matches(&self, date: Date) -> bool {
        let day = self.days.has(date.day());
        let weekday = self.weekdays.has(date.weekday().to_sunday_zero_offset());
        match (self.any_day, self.any_weekday) {
            (false, false) => day || weekday,
            _ => day && weekday,
        }
    }

    /// The first minute strictly after `after` the schedule matches;
    /// `None` when none comes within 5 years (`0 0 30 2 *`).
    pub fn next_after(&self, after: Timestamp) -> Option<Timestamp> {
        let start = after.to_zoned(TimeZone::UTC).datetime();
        // The next whole minute.
        let mut at = DateTime::from_parts(
            start.date(),
            Time::new(start.hour(), start.minute(), 0, 0).ok()?,
        )
        .checked_add(1.minute())
        .ok()?;
        let limit = start.date().checked_add(5.years()).ok()?;
        while at.date() <= limit {
            let date = at.date();
            if !self.months.has(date.month()) {
                let first = Date::new(date.year(), date.month(), 1).ok()?;
                at = first
                    .checked_add(1.month())
                    .ok()?
                    .to_datetime(Time::midnight());
                continue;
            }
            if !self.day_matches(date) {
                at = date
                    .checked_add(1.day())
                    .ok()?
                    .to_datetime(Time::midnight());
                continue;
            }
            if !self.hours.has(at.hour()) {
                at = DateTime::from_parts(date, Time::new(at.hour(), 0, 0, 0).ok()?)
                    .checked_add(1.hour())
                    .ok()?;
                continue;
            }
            if !self.minutes.has(at.minute()) {
                at = at.checked_add(1.minute()).ok()?;
                continue;
            }
            return at.to_zoned(TimeZone::UTC).ok().map(|z| z.timestamp());
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    fn next(schedule: &str, after: &str) -> String {
        Schedule::parse(schedule)
            .unwrap()
            .next_after(at(after))
            .unwrap()
            .to_string()
    }

    #[test]
    fn the_next_run_is_strictly_after() {
        assert_eq!(
            next("* * * * *", "2026-10-15T10:00:00Z"),
            "2026-10-15T10:01:00Z"
        );
        assert_eq!(
            next("* * * * *", "2026-10-15T10:00:30Z"),
            "2026-10-15T10:01:00Z"
        );
        assert_eq!(
            next("30 2 * * *", "2026-10-15T10:00:00Z"),
            "2026-10-16T02:30:00Z"
        );
        assert_eq!(
            next("*/15 * * * *", "2026-10-15T10:07:00Z"),
            "2026-10-15T10:15:00Z"
        );
        assert_eq!(
            next("0 0 1 * *", "2026-12-15T00:00:00Z"),
            "2027-01-01T00:00:00Z"
        );
        assert_eq!(
            next("0 9-17/4 * * *", "2026-10-15T14:00:00Z"),
            "2026-10-15T17:00:00Z"
        );
        assert_eq!(
            next("0 0 29 2 *", "2026-10-15T00:00:00Z"),
            "2028-02-29T00:00:00Z"
        );
    }

    #[test]
    fn days_of_week_count_from_sunday_and_or_with_days_of_month() {
        // 2026-10-15 is a Thursday.
        assert_eq!(
            next("0 0 * * 0", "2026-10-15T00:00:00Z"),
            "2026-10-18T00:00:00Z"
        );
        assert_eq!(
            next("0 0 * * 7", "2026-10-15T00:00:00Z"),
            "2026-10-18T00:00:00Z"
        );
        assert_eq!(
            next("0 0 * * 1-5", "2026-10-16T12:00:00Z"),
            "2026-10-19T00:00:00Z"
        );
        // The 20th, or a Saturday: whichever comes first.
        assert_eq!(
            next("0 0 20 * 6", "2026-10-15T00:00:00Z"),
            "2026-10-17T00:00:00Z"
        );
        // A field starting with `*` is unrestricted (Vixie cron): every
        // other day of the month AND Saturdays, not OR.
        assert_eq!(
            next("0 0 */2 * 6", "2026-10-15T00:00:00Z"),
            "2026-10-17T00:00:00Z"
        );
        assert_eq!(
            next("0 0 */2 * 6", "2026-10-17T00:00:00Z"),
            "2026-10-31T00:00:00Z"
        );
        // Mondays, on days 1-5 only (`*/1` in the day-of-week).
        assert_eq!(
            next("0 0 1-5 * */1", "2026-10-15T00:00:00Z"),
            "2026-11-01T00:00:00Z"
        );
    }

    #[test]
    fn invalid_expressions_say_why() {
        for (text, why) in [
            ("* * * *", "5 fields"),
            ("60 * * * *", "outside 0-59"),
            ("* * 0 * *", "outside 1-31"),
            ("*/0 * * * *", "step of 0"),
            ("5-1 * * * *", "empty"),
            ("@daily", "5 fields"),
            ("a * * * *", "not a number"),
        ] {
            let err = Schedule::parse(text).unwrap_err();
            assert!(err.0.contains(why), "{text}: {err}");
        }
        assert_eq!(
            Schedule::parse("0 0 30 2 *")
                .unwrap()
                .next_after(at("2026-10-15T00:00:00Z")),
            None
        );
    }
}
