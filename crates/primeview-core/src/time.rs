//! Dependency-free UTC calendar math (works identically on wasm and native).

pub const MS_PER_DAY: i64 = 86_400_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateTime {
    pub year: i32,
    pub month: u32, // 1..=12
    pub day: u32,   // 1..=31
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

impl DateTime {
    pub fn from_millis(ms: i64) -> Self {
        let days = ms.div_euclid(MS_PER_DAY);
        let rem = ms.rem_euclid(MS_PER_DAY) / 1000;
        let (year, month, day) = civil_from_days(days);
        Self {
            year,
            month,
            day,
            hour: (rem / 3600) as u32,
            minute: ((rem % 3600) / 60) as u32,
            second: (rem % 60) as u32,
        }
    }

    pub fn month_name(&self) -> &'static str {
        MONTHS[(self.month - 1) as usize]
    }
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 → (y, m, d).
pub fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((if m <= 2 { y + 1 } else { y }) as i32, m, d)
}

/// Inverse of [`civil_from_days`].
pub fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y as i64 - 1 } else { y as i64 };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Full timestamp for crosshair labels, e.g. `Tue 27 Nov '26 14:05`.
pub fn format_full(ms: i64, intraday: bool) -> String {
    let dt = DateTime::from_millis(ms);
    let wd = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"]
        [ms.div_euclid(MS_PER_DAY).rem_euclid(7) as usize];
    let yy = dt.year.rem_euclid(100);
    if intraday {
        format!(
            "{wd} {} {} '{yy:02} {:02}:{:02}",
            dt.day,
            dt.month_name(),
            dt.hour,
            dt.minute
        )
    } else {
        format!("{wd} {} {} '{yy:02}", dt.day, dt.month_name())
    }
}

/// Axis tick label: picks the coarsest meaningful unit that changed.
pub fn format_tick(ms: i64, tick_step_ms: i64) -> String {
    let dt = DateTime::from_millis(ms);
    if dt.month == 1 && dt.day == 1 && dt.hour == 0 && dt.minute == 0 {
        return dt.year.to_string();
    }
    if tick_step_ms >= 28 * MS_PER_DAY || (dt.day == 1 && dt.hour == 0 && dt.minute == 0) {
        return dt.month_name().to_string();
    }
    if tick_step_ms >= MS_PER_DAY || (dt.hour == 0 && dt.minute == 0) {
        return dt.day.to_string();
    }
    format!("{:02}:{:02}", dt.hour, dt.minute)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 2026-11-27
        let d = days_from_civil(2026, 11, 27);
        assert_eq!(civil_from_days(d), (2026, 11, 27));
        assert_eq!(civil_from_days(days_from_civil(2000, 2, 29)), (2000, 2, 29));
        let dt = DateTime::from_millis(d * MS_PER_DAY + 14 * 3_600_000 + 5 * 60_000);
        assert_eq!((dt.hour, dt.minute), (14, 5));
        assert_eq!(format_full(d * MS_PER_DAY, false), "Fri 27 Nov '26");
    }

    #[test]
    fn tick_labels() {
        let jan1 = days_from_civil(2026, 1, 1) * MS_PER_DAY;
        assert_eq!(format_tick(jan1, 3_600_000), "2026");
        let mar1 = days_from_civil(2026, 3, 1) * MS_PER_DAY;
        assert_eq!(format_tick(mar1, MS_PER_DAY), "Mar");
        assert_eq!(format_tick(mar1 + 3 * MS_PER_DAY, MS_PER_DAY), "4");
        assert_eq!(format_tick(mar1 + 90 * 60_000, 30 * 60_000), "01:30");
    }
}
