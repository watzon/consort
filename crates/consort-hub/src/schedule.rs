//! Schedule math for routines. Pure functions: no I/O, no clock.
//!
//! [`next_run_at`] finds the next run of a [`RoutineSchedule`]. The scheduler
//! and the routine handlers call it. [`validate_schedule`] checks a schedule
//! before the hub stores it.

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use consort_proto::model::RoutineSchedule;
use consort_proto::time::Timestamp;
use croner::Cron;
use croner::parser::{CronParser, Seconds, Year};

use crate::error::{HubError, Result};

const MS_PER_SECOND: i64 = 1_000;

/// The first run of `schedule` that is strictly after `after`.
///
/// `Cron` and `Interval` schedules always have a next run. A `Once` schedule
/// has none when its time is not after `after`. A cron time that does not
/// exist on a day (a daylight saving gap) runs at the first real minute after
/// the gap. A cron time that happens twice (a daylight saving overlap) runs
/// once, the first time.
pub(crate) fn next_run_at(
    schedule: &RoutineSchedule,
    after: Timestamp,
) -> Result<Option<Timestamp>> {
    match schedule {
        RoutineSchedule::Cron {
            expression,
            time_zone,
        } => next_cron_run(expression, time_zone, after).map(Some),
        RoutineSchedule::Interval { seconds } => {
            let ms = i64::try_from(*seconds)
                .ok()
                .and_then(|s| s.checked_mul(MS_PER_SECOND))
                .ok_or_else(|| {
                    HubError::invalid(format!(
                        "The interval of {seconds} seconds is too long. Use a shorter interval."
                    ))
                })?;
            Ok(Some(after.plus_millis(ms)))
        }
        RoutineSchedule::Once { at } => Ok((*at > after).then_some(*at)),
    }
}

/// Checks a schedule for a new or changed routine.
///
/// It runs the proto checks, then parses the cron expression and the time
/// zone. It does not look at the clock: a `Once` time in the past is valid
/// here, and the caller decides whether that is an error.
pub(crate) fn validate_schedule(schedule: &RoutineSchedule) -> Result<()> {
    schedule.validate()?;
    if let RoutineSchedule::Cron {
        expression,
        time_zone,
    } = schedule
    {
        parse_cron(expression)?;
        parse_zone(time_zone)?;
    }
    Ok(())
}

fn next_cron_run(expression: &str, time_zone: &str, after: Timestamp) -> Result<Timestamp> {
    let cron = parse_cron(expression)?;
    let zone = parse_zone(time_zone)?;
    // Cron times fall on whole seconds. A time is after `after` exactly when
    // it is after `after` rounded down to the second.
    let whole_second = after.as_millis().div_euclid(MS_PER_SECOND) * MS_PER_SECOND;
    let start: DateTime<Tz> = DateTime::<Utc>::from_timestamp_millis(whole_second)
        .ok_or_else(|| HubError::invalid("The time is out of range. Use a time near the present."))?
        .with_timezone(&zone);
    let next = cron.find_next_occurrence(&start, false).map_err(|err| {
        HubError::invalid(format!(
            "The cron expression `{expression}` has no run in the time zone {time_zone}: {err} \
             Change the expression."
        ))
    })?;
    Ok(Timestamp::from_millis(next.timestamp_millis()))
}

/// Parses a five-field expression. Seconds are always 0, and a year field is
/// not allowed.
fn parse_cron(expression: &str) -> Result<Cron> {
    CronParser::builder()
        .seconds(Seconds::Disallowed)
        .year(Year::Disallowed)
        .build()
        .parse(expression)
        .map_err(|err| {
            HubError::invalid(format!(
                "The cron expression `{expression}` is not valid: {err} \
                 Use five fields: minute, hour, day of month, month, day of week."
            ))
        })
}

pub(crate) fn parse_zone(time_zone: &str) -> Result<Tz> {
    time_zone.trim().parse::<Tz>().map_err(|_| {
        HubError::invalid(format!(
            "The time zone `{time_zone}` is not known. Use an IANA name such as America/Chicago."
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_proto::error::ErrorCode;

    fn cron(expression: &str, time_zone: &str) -> RoutineSchedule {
        RoutineSchedule::Cron {
            expression: expression.to_owned(),
            time_zone: time_zone.to_owned(),
        }
    }

    fn at(rfc3339: &str) -> Timestamp {
        Timestamp::parse_rfc3339(rfc3339).unwrap()
    }

    fn next(schedule: &RoutineSchedule, after: &str) -> String {
        next_run_at(schedule, at(after))
            .unwrap()
            .unwrap()
            .to_rfc3339()
    }

    #[test]
    fn cron_runs_in_the_given_time_zone() {
        // 08:00 in Chicago is 14:00 UTC in January (CST, UTC-6).
        let schedule = cron("0 8 * * *", "America/Chicago");
        assert_eq!(
            next(&schedule, "2026-01-15T00:00:00Z"),
            "2026-01-15T14:00:00Z"
        );
        // The next day, once 08:00 has passed.
        assert_eq!(
            next(&schedule, "2026-01-15T14:00:00Z"),
            "2026-01-16T14:00:00Z"
        );
        // 08:00 in Chicago is 13:00 UTC in July (CDT, UTC-5).
        assert_eq!(
            next(&schedule, "2026-07-15T00:00:00Z"),
            "2026-07-15T13:00:00Z"
        );
    }

    #[test]
    fn cron_time_is_strictly_after_the_start() {
        let schedule = cron("*/15 * * * *", "UTC");
        assert_eq!(
            next(&schedule, "2026-03-01T10:00:00Z"),
            "2026-03-01T10:15:00Z"
        );
        // A start inside the second, after the run, still moves on.
        let start = at("2026-03-01T10:15:00Z").plus_millis(400);
        let found = next_run_at(&schedule, start).unwrap().unwrap();
        assert_eq!(found.to_rfc3339(), "2026-03-01T10:30:00Z");
        // A start just before the run finds that run.
        let start = at("2026-03-01T10:15:00Z").plus_millis(-1);
        let found = next_run_at(&schedule, start).unwrap().unwrap();
        assert_eq!(found.to_rfc3339(), "2026-03-01T10:15:00Z");
    }

    #[test]
    fn cron_handles_a_start_before_1970() {
        let schedule = cron("0 0 * * *", "UTC");
        let found = next_run_at(&schedule, Timestamp::from_millis(-1))
            .unwrap()
            .unwrap();
        assert_eq!(found, Timestamp::EPOCH);
    }

    #[test]
    fn cron_uses_weekday_ranges() {
        // 2026-10-30 is a Friday. The next weekday run after Friday 08:00 is Monday.
        let schedule = cron("0 8 * * 1-5", "UTC");
        assert_eq!(
            next(&schedule, "2026-10-30T08:00:00Z"),
            "2026-11-02T08:00:00Z"
        );
    }

    #[test]
    fn cron_skips_a_daylight_saving_gap_without_looping() {
        // On 2026-03-08 in Chicago the clocks jump from 02:00 CST to 03:00 CDT.
        // A fixed 02:30 run does not exist that day. It runs at 03:00 CDT,
        // which is 08:00 UTC.
        let schedule = cron("30 2 * * *", "America/Chicago");
        assert_eq!(
            next(&schedule, "2026-03-08T00:00:00Z"),
            "2026-03-08T08:00:00Z"
        );
        // The day after, 02:30 CDT is 07:30 UTC.
        assert_eq!(
            next(&schedule, "2026-03-08T08:00:00Z"),
            "2026-03-09T07:30:00Z"
        );
    }

    #[test]
    fn cron_wildcard_runs_cross_a_gap_in_order() {
        // Every 30 minutes across the same gap. Runs inside the gap are skipped.
        let schedule = cron("*/30 * * * *", "America/Chicago");
        // 01:30 CST is 07:30 UTC. The next real run is 03:00 CDT, 08:00 UTC.
        assert_eq!(
            next(&schedule, "2026-03-08T07:30:00Z"),
            "2026-03-08T08:00:00Z"
        );
        assert_eq!(
            next(&schedule, "2026-03-08T08:00:00Z"),
            "2026-03-08T08:30:00Z"
        );
    }

    #[test]
    fn cron_runs_once_in_a_daylight_saving_overlap() {
        // On 2026-11-01 in Chicago the clocks go back from 02:00 CDT to 01:00 CST.
        // A fixed 01:30 run happens once, at the first 01:30 (CDT, 06:30 UTC).
        let schedule = cron("30 1 * * *", "America/Chicago");
        assert_eq!(
            next(&schedule, "2026-11-01T00:00:00Z"),
            "2026-11-01T06:30:00Z"
        );
        // After that run, the next one is on the next day (01:30 CST, 07:30 UTC).
        assert_eq!(
            next(&schedule, "2026-11-01T06:30:00Z"),
            "2026-11-02T07:30:00Z"
        );
    }

    #[test]
    fn cron_in_a_half_hour_zone() {
        // Asia/Kolkata is UTC+5:30 with no daylight saving.
        let schedule = cron("0 9 * * *", "Asia/Kolkata");
        assert_eq!(
            next(&schedule, "2026-06-01T00:00:00Z"),
            "2026-06-01T03:30:00Z"
        );
    }

    #[test]
    fn an_unknown_time_zone_is_invalid_and_named() {
        let schedule = cron("0 8 * * *", "Mars/Olympus");
        for err in [
            next_run_at(&schedule, Timestamp::EPOCH).unwrap_err(),
            validate_schedule(&schedule).unwrap_err(),
        ] {
            assert_eq!(err.code(), ErrorCode::Invalid);
            assert!(err.to_string().contains("Mars/Olympus"), "{err}");
        }
    }

    #[test]
    fn a_bad_expression_is_invalid_and_named() {
        for expression in [
            "61 * * * *",
            "* * * * foo",
            "not a cron at all",
            "99 99 99 99 99",
        ] {
            let schedule = cron(expression, "UTC");
            for err in [
                next_run_at(&schedule, Timestamp::EPOCH).unwrap_err(),
                validate_schedule(&schedule).unwrap_err(),
            ] {
                assert_eq!(err.code(), ErrorCode::Invalid, "{expression}");
                assert!(err.to_string().contains(expression), "{err}");
            }
        }
    }

    #[test]
    fn six_field_expressions_are_invalid() {
        // Seconds are not allowed. The proto check counts the fields.
        let schedule = cron("0 0 8 * * *", "UTC");
        assert_eq!(
            validate_schedule(&schedule).unwrap_err().code(),
            ErrorCode::Invalid
        );
        // The math refuses it too, in case a stored schedule skipped validation.
        assert_eq!(
            next_run_at(&schedule, Timestamp::EPOCH).unwrap_err().code(),
            ErrorCode::Invalid
        );
    }

    #[test]
    fn an_expression_that_never_matches_is_invalid() {
        // There is no 30 February.
        let schedule = cron("0 0 30 2 *", "UTC");
        let err = next_run_at(&schedule, at("2026-01-01T00:00:00Z")).unwrap_err();
        assert_eq!(err.code(), ErrorCode::Invalid);
        assert!(err.to_string().contains("0 0 30 2 *"), "{err}");
    }

    #[test]
    fn validate_accepts_good_schedules() {
        assert!(validate_schedule(&cron("0 8 * * 1-5", "America/Chicago")).is_ok());
        assert!(validate_schedule(&RoutineSchedule::Interval { seconds: 60 }).is_ok());
        // A time in the past is the caller's concern.
        assert!(
            validate_schedule(&RoutineSchedule::Once {
                at: Timestamp::EPOCH
            })
            .is_ok()
        );
    }

    #[test]
    fn validate_runs_the_proto_checks() {
        let err = validate_schedule(&RoutineSchedule::Interval { seconds: 59 }).unwrap_err();
        assert_eq!(err.code(), ErrorCode::Invalid);
        assert!(err.to_string().contains("schedule.seconds"), "{err}");
    }

    #[test]
    fn interval_adds_seconds_in_milliseconds() {
        let schedule = RoutineSchedule::Interval { seconds: 3_600 };
        let after = at("2026-10-29T09:00:00Z").plus_millis(250);
        let found = next_run_at(&schedule, after).unwrap().unwrap();
        assert_eq!(found.as_millis(), after.as_millis() + 3_600_000);
    }

    #[test]
    fn interval_that_overflows_is_invalid() {
        let schedule = RoutineSchedule::Interval { seconds: u64::MAX };
        let err = next_run_at(&schedule, Timestamp::EPOCH).unwrap_err();
        assert_eq!(err.code(), ErrorCode::Invalid);
    }

    #[test]
    fn once_in_the_future_runs_at_its_time() {
        let when = at("2026-12-01T09:00:00Z");
        let schedule = RoutineSchedule::Once { at: when };
        assert_eq!(
            next_run_at(&schedule, at("2026-11-01T00:00:00Z")).unwrap(),
            Some(when)
        );
    }

    #[test]
    fn once_in_the_past_or_now_has_no_next_run() {
        let when = at("2026-12-01T09:00:00Z");
        let schedule = RoutineSchedule::Once { at: when };
        assert_eq!(next_run_at(&schedule, when).unwrap(), None);
        assert_eq!(next_run_at(&schedule, when.plus_millis(1)).unwrap(), None);
        assert_eq!(
            next_run_at(&schedule, at("2027-01-01T00:00:00Z")).unwrap(),
            None
        );
    }
}
