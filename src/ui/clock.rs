//! Clock line in the style of the iOS lock screen (SPEC §5.2 v1.2):
//! bold time, regular date, following the UI language.

/// Local wall-clock time as Windows reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    pub hour: u16,
    pub minute: u16,
    pub day: u16,
    /// 1..=12
    pub month: u16,
    /// 0 = Sunday .. 6 = Saturday
    pub weekday: u16,
}

impl LocalTime {
    pub fn now() -> Self {
        // SAFETY: GetLocalTime only fills a SYSTEMTIME.
        let st = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        Self {
            hour: st.wHour,
            minute: st.wMinute,
            day: st.wDay,
            month: st.wMonth,
            weekday: st.wDayOfWeek,
        }
    }
}

/// Localized names and format (from `locales/*.toml`, `clock.*`).
pub struct ClockFormat<'a> {
    /// Comma separated, Sunday first.
    pub weekdays: &'a str,
    /// Comma separated, January first.
    pub months: &'a str,
    /// Placeholders `{weekday}`, `{day}`, `{month}`.
    pub date: &'a str,
    /// `24` for 24-hour time, otherwise 12-hour without AM/PM (like iOS).
    pub hours: &'a str,
}

/// Returns `(time, date)`, e.g. `("16:46", "Fr. 20. Nov.")`.
pub fn format(t: LocalTime, f: &ClockFormat) -> (String, String) {
    let hour = if f.hours.trim() == "24" {
        t.hour
    } else {
        match t.hour % 12 {
            0 => 12,
            h => h,
        }
    };
    let time = format!("{hour}:{:02}", t.minute);
    let pick = |list: &str, i: usize| {
        list.split(',')
            .nth(i)
            .map(str::trim)
            .unwrap_or_default()
            .to_string()
    };
    let date = f
        .date
        .replace("{weekday}", &pick(f.weekdays, t.weekday as usize))
        .replace("{day}", &t.day.to_string())
        .replace(
            "{month}",
            &pick(f.months, t.month.saturating_sub(1) as usize),
        );
    (time, date)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DE: ClockFormat = ClockFormat {
        weekdays: "So.,Mo.,Di.,Mi.,Do.,Fr.,Sa.",
        months: "Jan.,Feb.,März,Apr.,Mai,Juni,Juli,Aug.,Sept.,Okt.,Nov.,Dez.",
        date: "{weekday} {day}. {month}",
        hours: "24",
    };
    const EN: ClockFormat = ClockFormat {
        weekdays: "Sun,Mon,Tue,Wed,Thu,Fri,Sat",
        months: "Jan,Feb,Mar,Apr,May,Jun,Jul,Aug,Sep,Oct,Nov,Dec",
        date: "{weekday}, {month} {day}",
        hours: "12",
    };

    fn t(hour: u16, minute: u16) -> LocalTime {
        LocalTime {
            hour,
            minute,
            day: 20,
            month: 11,
            weekday: 5,
        }
    }

    #[test]
    fn german_lock_screen_style() {
        assert_eq!(
            format(t(16, 46), &DE),
            ("16:46".into(), "Fr. 20. Nov.".into())
        );
        assert_eq!(format(t(7, 5), &DE).0, "7:05");
    }

    #[test]
    fn english_twelve_hour() {
        assert_eq!(
            format(t(16, 46), &EN),
            ("4:46".into(), "Fri, Nov 20".into())
        );
        assert_eq!(format(t(0, 3), &EN).0, "12:03");
        assert_eq!(format(t(12, 0), &EN).0, "12:00");
    }
}
