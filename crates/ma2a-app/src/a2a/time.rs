//! ISO 8601 UTC timestamps, to and from Unix milliseconds, using the civil-date algorithms
//! of Howard Hinnant.

/// Days since 1970-01-01 of one proleptic Gregorian date.
const fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Days in one month of a proleptic Gregorian year.
const fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Formats Unix milliseconds as `YYYY-MM-DDTHH:MM:SS.mmmZ`.
pub(super) fn format(at_ms: u64) -> String {
    let (days, rest) = (at_ms / 86_400_000, at_ms % 86_400_000);
    let shifted = days + 719_468;
    let (era, day_of_era) = (shifted / 146_097, shifted % 146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    let (hours, minutes) = (rest / 3_600_000, rest / 60_000 % 60);
    let (seconds, millis) = (rest / 1_000 % 60, rest % 1_000);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}.{millis:03}Z")
}

/// Parses `YYYY-MM-DDTHH:MM:SS[.fraction]Z` into Unix milliseconds; `None` when malformed
/// or before 1970. A finer fraction rounds up to the next millisecond, since the result is a
/// lower bound on stored milliseconds: a task at `.123` is before `.1239`. Unix time has no
/// leap seconds, so a leap second, which only ever ends a UTC day at `23:59:60`, counts as
/// the start of the next day, whatever its fraction.
pub(super) fn parse(text: &str) -> Option<u64> {
    let text = text.strip_suffix('Z')?;
    let (date, clock) = text.split_once('T')?;
    let number = |part: &str, width: usize| {
        (part.len() == width && part.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| part.parse::<i64>().ok())
            .flatten()
    };
    let mut date = date.split('-');
    let (year, month, day) = (
        number(date.next()?, 4)?,
        number(date.next()?, 2)?,
        number(date.next()?, 2)?,
    );
    let (clock, fraction) = match clock.split_once('.') {
        Some((_, "")) => return None,
        Some(split) => split,
        None => (clock, ""),
    };
    let mut clock = clock.split(':');
    let (hours, minutes, seconds) = (
        number(clock.next()?, 2)?,
        number(clock.next()?, 2)?,
        number(clock.next()?, 2)?,
    );
    if date.next().is_some()
        || clock.next().is_some()
        || year < 1970
        || !(1..=12).contains(&month)
        || !(1..=days_in_month(year, month)).contains(&day)
        || hours > 23
        || minutes > 59
        || seconds > 60
        || (seconds == 60 && (hours, minutes) != (23, 59))
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let millis = format!("{fraction:0<3}").get(..3)?.parse::<i64>().ok()?;
    let finer = fraction
        .get(3..)
        .is_some_and(|rest| rest.bytes().any(|byte| byte != b'0'));
    // Every instant of a leap second comes before the next day's first millisecond.
    let within = if seconds == 60 {
        0
    } else {
        millis + i64::from(finer)
    };
    let seconds =
        days_from_civil(year, month, day) * 86_400 + hours * 3_600 + minutes * 60 + seconds;
    u64::try_from(seconds * 1_000 + within).ok()
}

#[cfg(test)]
mod tests {
    use super::{format, parse};

    #[test]
    fn timestamps_format_as_iso_8601_utc() {
        assert_eq!(format(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(format(951_782_400_123), "2000-02-29T00:00:00.123Z");
        assert_eq!(format(1_790_825_558_916), "2026-10-01T03:32:38.916Z");
    }

    #[test]
    fn timestamps_parse_back_with_or_without_a_fraction() {
        assert_eq!(parse("2026-10-01T03:32:38.916Z"), Some(1_790_825_558_916));
        assert_eq!(parse("2000-02-29T00:00:00Z"), Some(951_782_400_000));
        assert_eq!(parse("2000-02-29T00:00:00.1Z"), Some(951_782_400_100));
        assert_eq!(parse("2024-02-29T00:00:00Z"), Some(1_709_164_800_000));
        // A lower bound between two milliseconds rounds up, so no earlier task passes it.
        assert_eq!(parse("2026-01-01T00:00:00.123Z"), Some(1_767_225_600_123));
        assert_eq!(parse("2026-01-01T00:00:00.1230Z"), Some(1_767_225_600_123));
        assert_eq!(parse("2026-01-01T00:00:00.1239Z"), Some(1_767_225_600_124));
        assert_eq!(
            parse("2026-01-01T00:00:00.0000001Z"),
            Some(1_767_225_600_001)
        );
        // A day-end leap second, all of it, counts as the first millisecond of the next day:
        // every instant of it comes before that day's first millisecond.
        for leap in [
            "2016-12-31T23:59:60Z",
            "2016-12-31T23:59:60.5Z",
            "2016-12-31T23:59:60.9999Z",
        ] {
            assert_eq!(parse(leap), Some(1_483_228_800_000), "{leap}");
        }
        assert_eq!(parse("2026-12-31T23:59:59.9999Z"), Some(1_798_761_600_000));
        for malformed in [
            "2000-02-29",
            "2000-13-01T00:00:00Z",
            "1969-12-31T23:59:59Z",
            "2023-02-29T00:00:00Z",
            "1900-02-29T00:00:00Z",
            "2024-04-31T00:00:00Z",
            "2024-04-30T00:00:00.Z",
            // Before 1970, however close: neither rounding nor a leap second carries it over.
            "1969-12-31T23:59:59.9999Z",
            "1969-12-31T23:59:60Z",
            // A leap second only ends a UTC day.
            "2026-01-15T12:00:60Z",
            "2026-01-15T23:58:60Z",
        ] {
            assert_eq!(parse(malformed), None, "{malformed}");
        }
    }
}
