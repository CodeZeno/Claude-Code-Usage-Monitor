use super::*;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::Globalization::{
    GetDateFormatEx, GetTimeFormatEx, DATE_LONGDATE, DATE_SHORTDATE, ENUM_DATE_FORMATS_FLAGS,
    TIME_FORMAT_FLAGS, TIME_NOSECONDS,
};
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTimeEx};

const WINDOWS_TO_UNIX_SECONDS: u64 = 11_644_473_600;
const TICKS_PER_SECOND: u64 = 10_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DateTimeParts {
    pub(super) year: u16,
    pub(super) month: u16,
    pub(super) day: u16,
    /// Monday is 0 and Sunday is 6, matching the Theme Studio examples.
    pub(super) weekday: u16,
    pub(super) hour: u16,
    pub(super) minute: u16,
    pub(super) second: u16,
}

pub(super) fn timestamp_parts(unix: f64, local: bool) -> Option<DateTimeParts> {
    let value = timestamp_system_time(unix, local)?;
    Some(DateTimeParts {
        year: value.wYear,
        month: value.wMonth,
        day: value.wDay,
        weekday: (value.wDayOfWeek + 6) % 7,
        hour: value.wHour,
        minute: value.wMinute,
        second: value.wSecond,
    })
}

pub(super) fn format_timestamp(unix: f64, format: &str, context: &DataContext) -> Option<String> {
    let normalized = format.trim().to_ascii_lowercase();
    let (utc, format) = normalized
        .strip_prefix("utc_")
        .map_or((false, normalized.as_str()), |format| (true, format));
    if !is_timestamp_format(format) {
        return None;
    }
    let Some(value) = timestamp_system_time(unix, !utc) else {
        return Some("--".into());
    };
    let locale = context.get_string("i18n.locale").unwrap_or("en");
    let result = match format {
        "weekday_2" => {
            date_pattern(&value, "ddd", locale).map(|value| value.chars().take(2).collect())
        }
        "weekday_short" => date_pattern(&value, "ddd", locale),
        "weekday_long" => date_pattern(&value, "dddd", locale),
        "day" => date_pattern(&value, "d", locale),
        "day_2" => date_pattern(&value, "dd", locale),
        "month" => date_pattern(&value, "M", locale),
        "month_2" => date_pattern(&value, "MM", locale),
        "month_short" => date_pattern(&value, "MMM", locale),
        "month_long" => date_pattern(&value, "MMMM", locale),
        "year_2" => date_pattern(&value, "yy", locale),
        "year" => date_pattern(&value, "yyyy", locale),
        "date" | "date_short" => date_default(&value, DATE_SHORTDATE, locale),
        "date_long" => date_default(&value, DATE_LONGDATE, locale),
        "time" | "time_short" => time_default(&value, TIME_NOSECONDS, locale),
        "time_seconds" => time_default(&value, TIME_FORMAT_FLAGS(0), locale),
        "time_24" => time_pattern(&value, "HH':'mm", locale),
        "time_24_seconds" => time_pattern(&value, "HH':'mm':'ss", locale),
        "time_12" => time_pattern(&value, "h':'mm tt", locale),
        "time_12_seconds" => time_pattern(&value, "h':'mm':'ss tt", locale),
        "am_pm" => time_pattern(&value, "tt", locale),
        "datetime" | "datetime_short" => combine(
            date_default(&value, DATE_SHORTDATE, locale),
            time_default(&value, TIME_NOSECONDS, locale),
        ),
        "datetime_long" => combine(
            date_default(&value, DATE_LONGDATE, locale),
            time_default(&value, TIME_FORMAT_FLAGS(0), locale),
        ),
        "iso_date" => Some(format!(
            "{:04}-{:02}-{:02}",
            value.wYear, value.wMonth, value.wDay
        )),
        "iso_time" => Some(format!(
            "{:02}:{:02}:{:02}",
            value.wHour, value.wMinute, value.wSecond
        )),
        "iso_datetime" => Some(format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{}",
            value.wYear,
            value.wMonth,
            value.wDay,
            value.wHour,
            value.wMinute,
            value.wSecond,
            if utc { "Z" } else { "" }
        )),
        _ => None,
    };
    Some(result.unwrap_or_else(|| "--".into()))
}

fn is_timestamp_format(format: &str) -> bool {
    matches!(
        format,
        "weekday_2"
            | "weekday_short"
            | "weekday_long"
            | "day"
            | "day_2"
            | "month"
            | "month_2"
            | "month_short"
            | "month_long"
            | "year_2"
            | "year"
            | "date"
            | "date_short"
            | "date_long"
            | "time"
            | "time_short"
            | "time_seconds"
            | "time_24"
            | "time_24_seconds"
            | "time_12"
            | "time_12_seconds"
            | "am_pm"
            | "datetime"
            | "datetime_short"
            | "datetime_long"
            | "iso_date"
            | "iso_time"
            | "iso_datetime"
    )
}

fn timestamp_system_time(unix: f64, local: bool) -> Option<SYSTEMTIME> {
    if !unix.is_finite() || unix <= 0.0 {
        return None;
    }
    let seconds = unix.floor() as u64;
    let milliseconds = ((unix - seconds as f64) * 1000.0).floor() as u64;
    let ticks = seconds
        .checked_add(WINDOWS_TO_UNIX_SECONDS)?
        .checked_mul(TICKS_PER_SECOND)?
        .checked_add(milliseconds.min(999) * 10_000)?;
    let file_time = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut utc = SYSTEMTIME::default();
    unsafe { FileTimeToSystemTime(&file_time, &mut utc).ok()? };
    if !local {
        return Some(utc);
    }
    let mut local = SYSTEMTIME::default();
    unsafe { SystemTimeToTzSpecificLocalTimeEx(None, &utc, &mut local).ok()? };
    Some(local)
}

fn date_default(
    value: &SYSTEMTIME,
    flags: ENUM_DATE_FORMATS_FLAGS,
    locale: &str,
) -> Option<String> {
    format_date(value, flags, None, locale)
}

fn date_pattern(value: &SYSTEMTIME, pattern: &str, locale: &str) -> Option<String> {
    format_date(value, ENUM_DATE_FORMATS_FLAGS(0), Some(pattern), locale)
}

fn format_date(
    value: &SYSTEMTIME,
    flags: ENUM_DATE_FORMATS_FLAGS,
    pattern: Option<&str>,
    locale: &str,
) -> Option<String> {
    let locale = wide(locale);
    let pattern = pattern.map(wide);
    let mut output = [0_u16; 128];
    let count = unsafe {
        GetDateFormatEx(
            PCWSTR(locale.as_ptr()),
            flags,
            Some(value),
            pattern
                .as_ref()
                .map_or(PCWSTR::null(), |value| PCWSTR(value.as_ptr())),
            Some(&mut output),
            PCWSTR::null(),
        )
    };
    wide_result(&output, count)
}

fn time_default(value: &SYSTEMTIME, flags: TIME_FORMAT_FLAGS, locale: &str) -> Option<String> {
    format_time(value, flags, None, locale)
}

fn time_pattern(value: &SYSTEMTIME, pattern: &str, locale: &str) -> Option<String> {
    format_time(value, TIME_FORMAT_FLAGS(0), Some(pattern), locale)
}

fn format_time(
    value: &SYSTEMTIME,
    flags: TIME_FORMAT_FLAGS,
    pattern: Option<&str>,
    locale: &str,
) -> Option<String> {
    let locale = wide(locale);
    let pattern = pattern.map(wide);
    let mut output = [0_u16; 128];
    let count = unsafe {
        GetTimeFormatEx(
            PCWSTR(locale.as_ptr()),
            flags,
            Some(value),
            pattern
                .as_ref()
                .map_or(PCWSTR::null(), |value| PCWSTR(value.as_ptr())),
            Some(&mut output),
        )
    };
    wide_result(&output, count)
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn wide_result(value: &[u16], count: i32) -> Option<String> {
    let count = usize::try_from(count).ok()?.checked_sub(1)?;
    Some(String::from_utf16_lossy(value.get(..count)?))
}

fn combine(first: Option<String>, second: Option<String>) -> Option<String> {
    Some(format!("{} {}", first?, second?))
}
