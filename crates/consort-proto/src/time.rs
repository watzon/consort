//! Timestamps.
//!
//! All timestamps are UTC, stored and sent as Unix milliseconds. A
//! [`Timestamp`] serializes as an integer. It deserializes from an integer or
//! from an RFC 3339 string, so a model can write `"2026-10-29T09:00:00Z"` in a
//! hub tool argument.

use std::borrow::Cow;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A point in time as Unix milliseconds, UTC.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Timestamp(i64);

/// The error when a string is not an RFC 3339 time.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` is not an RFC 3339 time such as 2026-10-29T09:00:00Z")]
pub struct TimestampParseError(pub String);

const MS_PER_SECOND: i64 = 1_000;
const MS_PER_DAY: i64 = 86_400_000;

impl Timestamp {
    /// The Unix epoch.
    pub const EPOCH: Self = Self(0);

    /// Makes a timestamp from Unix milliseconds.
    pub const fn from_millis(ms: i64) -> Self {
        Self(ms)
    }

    /// The Unix milliseconds.
    pub const fn as_millis(self) -> i64 {
        self.0
    }

    /// The current time from the system clock. A clock before 1970 gives
    /// [`Timestamp::EPOCH`].
    pub fn now() -> Self {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
            .unwrap_or(0);
        Self(ms)
    }

    /// This time plus a number of milliseconds, saturating at the limits.
    pub const fn plus_millis(self, ms: i64) -> Self {
        Self(self.0.saturating_add(ms))
    }

    /// Formats as RFC 3339 in UTC with second precision, for example
    /// `2026-10-28T21:49:00Z`. This is the form in coordinator turn input.
    pub fn to_rfc3339(self) -> String {
        let days = self.0.div_euclid(MS_PER_DAY);
        let ms_of_day = self.0.rem_euclid(MS_PER_DAY);
        let (year, month, day) = civil_from_days(days);
        let secs = ms_of_day / MS_PER_SECOND;
        format!(
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
            secs / 3600,
            (secs / 60) % 60,
            secs % 60
        )
    }

    /// Parses an RFC 3339 time such as `2026-10-29T09:00:00Z`,
    /// `2026-10-29T09:00:00.250+02:00` or `2026-10-29 09:00:00z`.
    /// Fractions past milliseconds are dropped.
    pub fn parse_rfc3339(s: &str) -> Result<Self, TimestampParseError> {
        parse_rfc3339(s).ok_or_else(|| TimestampParseError(s.to_owned()))
    }
}

impl fmt::Debug for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Timestamp({} = {})", self.0, self.to_rfc3339())
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_rfc3339())
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_i64(self.0)
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TimestampVisitor;

        impl Visitor<'_> for TimestampVisitor {
            type Value = Timestamp;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("Unix milliseconds or an RFC 3339 time")
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Timestamp, E> {
                Ok(Timestamp(v))
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Timestamp, E> {
                i64::try_from(v)
                    .map(Timestamp)
                    .map_err(|_| E::custom("timestamp is out of range"))
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Timestamp, E> {
                Timestamp::parse_rfc3339(v).map_err(E::custom)
            }
        }

        deserializer.deserialize_any(TimestampVisitor)
    }
}

impl JsonSchema for Timestamp {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("Timestamp")
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "format": "date-time",
            "description": "An RFC 3339 time, for example 2026-10-29T09:00:00Z.",
        })
    }
}

/// Days since 1970-01-01 to (year, month, day). Howard Hinnant's algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    // `m` is in 1..=12 and `d` in 1..=31 by construction.
    (year, m as u32, d as u32)
}

/// (year, month, day) to days since 1970-01-01. Howard Hinnant's algorithm.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
            if leap { 29 } else { 28 }
        }
    }
}

fn digits(bytes: &[u8]) -> Option<i64> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(
        bytes
            .iter()
            .fold(0_i64, |acc, b| acc * 10 + i64::from(b - b'0')),
    )
}

fn parse_rfc3339(s: &str) -> Option<Timestamp> {
    let b = s.as_bytes();
    if b.len() < 20 {
        return None;
    }
    if b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    if b[13] != b':' || b[16] != b':' {
        return None;
    }
    let year = digits(&b[0..4])?;
    let month = digits(&b[5..7])?;
    let day = digits(&b[8..10])?;
    let hour = digits(&b[11..13])?;
    let minute = digits(&b[14..16])?;
    let second = digits(&b[17..19])?;
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return None;
    }
    // A leap second (60) is read as the last second of the minute.
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let second = second.min(59);

    let mut rest = &b[19..];
    let mut millis = 0_i64;
    if let Some((&b'.', frac)) = rest.split_first() {
        let len = frac.iter().take_while(|c| c.is_ascii_digit()).count();
        if len == 0 {
            return None;
        }
        let mut ms_digits = [b'0'; 3];
        for (slot, digit) in ms_digits.iter_mut().zip(&frac[..len]) {
            *slot = *digit;
        }
        millis = digits(&ms_digits)?;
        rest = &frac[len..];
    }

    let offset_minutes = match rest {
        [b'Z' | b'z'] => 0,
        [sign @ (b'+' | b'-'), h1, h2, b':', m1, m2] => {
            let oh = digits(&[*h1, *h2])?;
            let om = digits(&[*m1, *m2])?;
            if oh > 23 || om > 59 {
                return None;
            }
            let total = oh * 60 + om;
            if *sign == b'-' { -total } else { total }
        }
        _ => return None,
    };

    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + hour * 3_600 + minute * 60 + second - offset_minutes * 60;
    Some(Timestamp(secs * MS_PER_SECOND + millis))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_times() {
        assert_eq!(Timestamp::EPOCH.to_rfc3339(), "1970-01-01T00:00:00Z");
        // 2026-10-28T21:49:00Z
        let t = Timestamp::from_millis(1_793_224_140_000);
        assert_eq!(t.to_rfc3339(), "2026-10-28T21:49:00Z");
        assert_eq!(
            Timestamp::from_millis(951_782_400_000).to_rfc3339(),
            "2000-02-29T00:00:00Z"
        );
        assert_eq!(
            Timestamp::from_millis(-1).to_rfc3339(),
            "1969-12-31T23:59:59Z"
        );
    }

    #[test]
    fn parses_and_round_trips() {
        let t = Timestamp::parse_rfc3339("2026-10-28T21:49:00Z").unwrap();
        assert_eq!(t.as_millis(), 1_793_224_140_000);
        assert_eq!(Timestamp::parse_rfc3339(&t.to_rfc3339()).unwrap(), t);

        let with_offset = Timestamp::parse_rfc3339("2026-10-28T23:49:00.250+02:00").unwrap();
        assert_eq!(with_offset.as_millis(), 1_793_224_140_250);

        let negative = Timestamp::parse_rfc3339("2026-10-28T16:49:00-05:00").unwrap();
        assert_eq!(negative, t);

        let micros = Timestamp::parse_rfc3339("2026-10-28T21:49:00.123456Z").unwrap();
        assert_eq!(micros.as_millis(), 1_793_224_140_123);

        for millis in [
            0_i64,
            1,
            86_399_999,
            951_782_400_000,
            4_102_444_800_000,
            -86_400_001,
        ] {
            let t = Timestamp::from_millis(millis - millis.rem_euclid(1000));
            assert_eq!(Timestamp::parse_rfc3339(&t.to_rfc3339()).unwrap(), t);
        }
    }

    #[test]
    fn rejects_bad_times() {
        for bad in [
            "",
            "2026-10-28",
            "2026-10-28T21:49:00",
            "2026-13-01T00:00:00Z",
            "2026-02-29T00:00:00Z",
            "2026-10-28T24:00:00Z",
            "2026-10-28T21:49:00.Z",
            "2026-10-28T21:49:00+0200",
            "tomorrow at nine",
        ] {
            assert!(Timestamp::parse_rfc3339(bad).is_err(), "{bad} should fail");
        }
    }

    #[test]
    fn serde_writes_integers_and_reads_both() {
        let t = Timestamp::from_millis(1_793_224_140_000);
        assert_eq!(serde_json::to_string(&t).unwrap(), "1793224140000");
        assert_eq!(
            serde_json::from_str::<Timestamp>("1793224140000").unwrap(),
            t
        );
        assert_eq!(
            serde_json::from_str::<Timestamp>("\"2026-10-28T21:49:00Z\"").unwrap(),
            t
        );
        assert!(serde_json::from_str::<Timestamp>("\"soon\"").is_err());
    }

    #[test]
    fn now_is_after_2025() {
        assert!(Timestamp::now().as_millis() > 1_735_689_600_000);
    }
}
