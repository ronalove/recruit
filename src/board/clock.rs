// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
//! Times read from the conversation files, and times shown.

/// Seconds since the epoch of an ISO 8601 time: `2026-10-07T10:28:51.123Z`, or with an offset (`+02:00`).
pub fn iso_epoch(text: &str) -> Option<i64> {
    let (date, time) = text.trim().split_once('T')?;
    let mut parts = date.splitn(3, '-').map(|p| p.parse::<i64>().ok());
    let (year, month, day) = (parts.next()??, parts.next()??, parts.next()??);
    let (time, offset) = match time.find(['Z', '+', '-']) {
        Some(at) if &time[at..] == "Z" => (&time[..at], 0),
        Some(at) => {
            let sign = if time[at..].starts_with('-') { -1 } else { 1 };
            let (h, m) = time[at + 1..].split_once(':')?;
            (&time[..at], sign * (h.parse::<i64>().ok()? * 3600 + m.parse::<i64>().ok()? * 60))
        }
        None => (time, 0),
    };
    let mut clock = time.splitn(3, ':');
    let hours: i64 = clock.next()?.parse().ok()?;
    let minutes: i64 = clock.next()?.parse().ok()?;
    let seconds: f64 = clock.next()?.parse().ok()?;
    Some(days_from_civil(year, month, day) * 86_400 + hours * 3600 + minutes * 60 + seconds as i64 - offset)
}

/// Days since 1970-01-01 of a date of the proleptic Gregorian calendar (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `HH:MM:SS` in local time.
// libc marks musl's `time_t` deprecated over its size on 32-bit targets; it is 64 bits on the ones recruit ships.
#[allow(deprecated)]
pub(super) fn clock(epoch: i64) -> String {
    let time = epoch as libc::time_t;
    // SAFETY: localtime_r only writes the `tm` it is given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&time, &mut tm) }.is_null() {
        return String::new();
    }
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn iso_times() {
        assert_eq!(iso_epoch("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(iso_epoch("2026-10-07T10:28:51.123Z"), Some(1_791_368_931));
        assert_eq!(iso_epoch("2026-10-07T12:28:51+02:00"), Some(1_791_368_931));
        assert_eq!(iso_epoch("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(iso_epoch("hier"), None);
    }
}
