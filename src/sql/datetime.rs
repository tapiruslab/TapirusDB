//! Pure-Rust SQLite-compatible date and time scalar evaluation utilities.
//!
//! Uses proleptic Gregorian calendar arithmetic (`#![forbid(unsafe_code)]`,
//! `wasm32-unknown-unknown` and `edition-lite` compatible via `web_time`).

use crate::error::{Error, Result};
use crate::traits::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DateTimeParts {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    millis: u32,
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

/// Howard Hinnant civil-from-days algorithm (days since 1970-01-01).
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (mp + if mp < 10 { 3 } else { -9 }) as u32;
    let year = (y + i64::from(m <= 2)) as i32;
    (year, m, d)
}

/// Howard Hinnant days-from-civil algorithm (days since 1970-01-01).
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let y = i64::from(year) - i64::from(month <= 2);
    let m = i64::from(month);
    let d = i64::from(day);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn unix_ms_to_parts(total_ms: i64) -> DateTimeParts {
    let total_secs = total_ms.div_euclid(1000);
    let millis = total_ms.rem_euclid(1000) as u32;
    let days = total_secs.div_euclid(86_400);
    let rem = total_secs.rem_euclid(86_400);
    let hour = (rem / 3600) as u32;
    let minute = ((rem % 3600) / 60) as u32;
    let second = (rem % 60) as u32;
    let (year, month, day) = civil_from_days(days);
    DateTimeParts {
        year,
        month,
        day,
        hour,
        minute,
        second,
        millis,
    }
}

fn parts_to_unix_ms(p: DateTimeParts) -> i64 {
    let days = days_from_civil(p.year, p.month, p.day);
    let secs =
        days * 86_400 + i64::from(p.hour) * 3600 + i64::from(p.minute) * 60 + i64::from(p.second);
    secs * 1000 + i64::from(p.millis)
}

fn now_unix_ms() -> i64 {
    match web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH) {
        Ok(d) => (d.as_secs() as i64) * 1000 + i64::from(d.subsec_millis()),
        Err(_) => 0,
    }
}

fn parse_hms(s: &str) -> Option<(u32, u32, u32, u32)> {
    let trimmed = s.trim_end_matches('Z').trim_end_matches("+00:00").trim();
    let (hms, frac) = match trimmed.split_once('.') {
        Some((main, f)) => {
            let digits: String = f.chars().take_while(|c| c.is_ascii_digit()).collect();
            let mut padded = digits;
            while padded.len() < 3 {
                padded.push('0');
            }
            let ms = padded[..3].parse::<u32>().ok()?;
            (main, ms)
        }
        None => (trimmed, 0),
    };
    let mut parts = hms.split(':');
    let hour: u32 = parts.next()?.parse().ok()?;
    let minute: u32 = parts.next()?.parse().ok()?;
    let second: u32 = parts.next().map_or(Some(0), |sec| sec.parse().ok())?;
    if parts.next().is_some() || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    Some((hour, minute, second, frac))
}

fn parse_ymd(s: &str) -> Option<(i32, u32, u32)> {
    let mut parts = s.trim().split('-');
    let year: i32 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) {
        return None;
    }
    if day < 1 || day > days_in_month(year, month) {
        return None;
    }
    Some((year, month, day))
}

fn parse_timestring(val: &Value, use_unixepoch: bool) -> Option<DateTimeParts> {
    if use_unixepoch {
        let secs_f = match val {
            Value::Integer(i) => *i as f64,
            Value::Real(f) => *f,
            Value::Text(s) => s.trim().parse::<f64>().ok()?,
            _ => return None,
        };
        return Some(unix_ms_to_parts((secs_f * 1000.0).round() as i64));
    }

    match val {
        Value::Null => None,
        Value::Integer(jd) => {
            let ms = ((*jd as f64 - 2_440_587.5) * 86_400_000.0).round() as i64;
            Some(unix_ms_to_parts(ms))
        }
        Value::Real(jd) => {
            let ms = ((*jd - 2_440_587.5) * 86_400_000.0).round() as i64;
            Some(unix_ms_to_parts(ms))
        }
        Value::Text(raw) => {
            let s = raw.trim();
            if s.eq_ignore_ascii_case("now") {
                return Some(unix_ms_to_parts(now_unix_ms()));
            }
            if let Some((date_part, time_part)) = s.split_once('T').or_else(|| s.split_once(' ')) {
                let (year, month, day) = parse_ymd(date_part)?;
                let (hour, minute, second, millis) = parse_hms(time_part)?;
                Some(DateTimeParts {
                    year,
                    month,
                    day,
                    hour,
                    minute,
                    second,
                    millis,
                })
            } else if s.contains(':') {
                let (hour, minute, second, millis) = parse_hms(s)?;
                Some(DateTimeParts {
                    year: 2000,
                    month: 1,
                    day: 1,
                    hour,
                    minute,
                    second,
                    millis,
                })
            } else if let Some((year, month, day)) = parse_ymd(s) {
                Some(DateTimeParts {
                    year,
                    month,
                    day,
                    hour: 0,
                    minute: 0,
                    second: 0,
                    millis: 0,
                })
            } else {
                None
            }
        }
        _ => None,
    }
}

fn shift_months(parts: DateTimeParts, delta_months: i32) -> DateTimeParts {
    let total_months = (parts.year * 12 + (parts.month as i32 - 1)) + delta_months;
    let new_year = total_months.div_euclid(12);
    let new_month = (total_months.rem_euclid(12) + 1) as u32;
    let max_d = days_in_month(new_year, new_month);
    DateTimeParts {
        year: new_year,
        month: new_month,
        day: parts.day.min(max_d),
        ..parts
    }
}

fn apply_modifier(mut parts: DateTimeParts, modifier: &str) -> Result<DateTimeParts> {
    let m = modifier.trim().to_ascii_lowercase();
    if m.is_empty() || m == "unixepoch" || m == "utc" {
        return Ok(parts);
    }
    match m.as_str() {
        "start of day" => {
            parts.hour = 0;
            parts.minute = 0;
            parts.second = 0;
            parts.millis = 0;
            return Ok(parts);
        }
        "start of month" => {
            parts.day = 1;
            parts.hour = 0;
            parts.minute = 0;
            parts.second = 0;
            parts.millis = 0;
            return Ok(parts);
        }
        "start of year" => {
            parts.month = 1;
            parts.day = 1;
            parts.hour = 0;
            parts.minute = 0;
            parts.second = 0;
            parts.millis = 0;
            return Ok(parts);
        }
        _ => {}
    }

    let mut tokens = m.split_whitespace();
    if let (Some(num_str), Some(unit)) = (tokens.next(), tokens.next())
        && tokens.next().is_none()
        && let Ok(val) = num_str.parse::<f64>()
    {
        match unit {
            "second" | "seconds" => {
                let ms = parts_to_unix_ms(parts) + (val * 1000.0).round() as i64;
                return Ok(unix_ms_to_parts(ms));
            }
            "minute" | "minutes" => {
                let ms = parts_to_unix_ms(parts) + (val * 60_000.0).round() as i64;
                return Ok(unix_ms_to_parts(ms));
            }
            "hour" | "hours" => {
                let ms = parts_to_unix_ms(parts) + (val * 3_600_000.0).round() as i64;
                return Ok(unix_ms_to_parts(ms));
            }
            "day" | "days" => {
                let ms = parts_to_unix_ms(parts) + (val * 86_400_000.0).round() as i64;
                return Ok(unix_ms_to_parts(ms));
            }
            "month" | "months" => {
                return Ok(shift_months(parts, val.trunc() as i32));
            }
            "year" | "years" => {
                return Ok(shift_months(parts, (val.trunc() as i32) * 12));
            }
            _ => {}
        }
    }

    Err(Error::SqlSyntax(format!(
        "Unsupported datetime modifier '{modifier}'"
    )))
}

fn resolve_datetime_args(args: &[Value]) -> Result<Option<DateTimeParts>> {
    let Some(first) = args.first() else {
        return Ok(Some(unix_ms_to_parts(now_unix_ms())));
    };
    if first.is_null() {
        return Ok(None);
    }
    let modifiers = &args[1..];
    let has_unixepoch = modifiers.iter().any(|m| match m {
        Value::Text(s) => s.trim().eq_ignore_ascii_case("unixepoch"),
        _ => false,
    });
    let Some(mut parts) = parse_timestring(first, has_unixepoch) else {
        return Ok(None);
    };
    for m in modifiers {
        match m {
            Value::Null => return Ok(None),
            Value::Text(s) => parts = apply_modifier(parts, s)?,
            other => parts = apply_modifier(parts, &other.to_string())?,
        }
    }
    Ok(Some(parts))
}

/// Evaluate SQLite `DATE(timestring, [modifier...])` -> `'YYYY-MM-DD'`.
pub(crate) fn eval_date(args: &[Value]) -> Result<Value> {
    let Some(p) = resolve_datetime_args(args)? else {
        return Ok(Value::Null);
    };
    Ok(Value::Text(format!(
        "{:04}-{:02}-{:02}",
        p.year, p.month, p.day
    )))
}

/// Evaluate SQLite `TIME(timestring, [modifier...])` -> `'HH:MM:SS'`.
pub(crate) fn eval_time(args: &[Value]) -> Result<Value> {
    let Some(p) = resolve_datetime_args(args)? else {
        return Ok(Value::Null);
    };
    Ok(Value::Text(format!(
        "{:02}:{:02}:{:02}",
        p.hour, p.minute, p.second
    )))
}

/// Evaluate SQLite `DATETIME(timestring, [modifier...])` -> `'YYYY-MM-DD HH:MM:SS'`.
pub(crate) fn eval_datetime(args: &[Value]) -> Result<Value> {
    let Some(p) = resolve_datetime_args(args)? else {
        return Ok(Value::Null);
    };
    Ok(Value::Text(format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        p.year, p.month, p.day, p.hour, p.minute, p.second
    )))
}

/// Evaluate SQLite `UNIXEPOCH(timestring, [modifier...])` -> integer seconds.
pub(crate) fn eval_unixepoch(args: &[Value]) -> Result<Value> {
    let Some(p) = resolve_datetime_args(args)? else {
        return Ok(Value::Null);
    };
    Ok(Value::Integer(parts_to_unix_ms(p).div_euclid(1000)))
}

/// Evaluate SQLite `JULIANDAY(timestring, [modifier...])` -> float Julian day.
pub(crate) fn eval_julianday(args: &[Value]) -> Result<Value> {
    let Some(p) = resolve_datetime_args(args)? else {
        return Ok(Value::Null);
    };
    let ms = parts_to_unix_ms(p) as f64;
    Ok(Value::Real(ms / 86_400_000.0 + 2_440_587.5))
}

/// Evaluate SQLite `STRFTIME(format, timestring, [modifier...])`.
pub(crate) fn eval_strftime(args: &[Value]) -> Result<Value> {
    let Some(fmt_val) = args.first() else {
        return Ok(Value::Null);
    };
    if fmt_val.is_null() {
        return Ok(Value::Null);
    }
    let fmt = match fmt_val {
        Value::Text(s) => s.clone(),
        other => other.to_string(),
    };
    let Some(p) = resolve_datetime_args(&args[1..])? else {
        return Ok(Value::Null);
    };

    let days_since_epoch = days_from_civil(p.year, p.month, p.day);
    // 1970-01-01 was Thursday (4); Sunday=0..Saturday=6
    let weekday = (days_since_epoch + 4).rem_euclid(7) as u32;
    let jan1_days = days_from_civil(p.year, 1, 1);
    let day_of_year = (days_since_epoch - jan1_days + 1) as u32;
    let week_sunday_first = (day_of_year - 1 + ((7 - weekday) % 7)) / 7;
    let unix_secs = parts_to_unix_ms(p).div_euclid(1000);

    let mut out = String::with_capacity(fmt.len() + 16);
    let mut chars = fmt.chars();
    while let Some(ch) = chars.next() {
        if ch == '%' {
            match chars.next() {
                Some('Y') => out.push_str(&format!("{:04}", p.year)),
                Some('m') => out.push_str(&format!("{:02}", p.month)),
                Some('d') => out.push_str(&format!("{:02}", p.day)),
                Some('H') => out.push_str(&format!("{:02}", p.hour)),
                Some('M') => out.push_str(&format!("{:02}", p.minute)),
                Some('S') => out.push_str(&format!("{:02}", p.second)),
                Some('f') => out.push_str(&format!("{:02}.{:03}", p.second, p.millis)),
                Some('w') => out.push_str(&weekday.to_string()),
                Some('W') => out.push_str(&format!("{week_sunday_first:02}")),
                Some('j') => out.push_str(&format!("{day_of_year:03}")),
                Some('s') => out.push_str(&unix_secs.to_string()),
                Some('%') => out.push('%'),
                Some(other) => {
                    out.push('%');
                    out.push(other);
                }
                None => out.push('%'),
            }
        } else {
            out.push(ch);
        }
    }
    Ok(Value::Text(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_date_time_strftime_and_modifiers() {
        assert_eq!(
            eval_date(&[Value::Text("2026-11-16 14:35:09".into())]).unwrap(),
            Value::Text("2026-11-16".into())
        );
        assert_eq!(
            eval_time(&[Value::Text("2026-11-16 14:35:09".into())]).unwrap(),
            Value::Text("14:35:09".into())
        );
        assert_eq!(
            eval_datetime(&[
                Value::Text("2026-01-31 10:00:00".into()),
                Value::Text("+1 month".into()),
                Value::Text("+2 hours".into()),
            ])
            .unwrap(),
            Value::Text("2026-02-28 12:00:00".into())
        );
        assert_eq!(
            eval_strftime(&[
                Value::Text("%Y/%m/%d %H:%M".into()),
                Value::Integer(1_700_000_000),
                Value::Text("unixepoch".into()),
            ])
            .unwrap(),
            Value::Text("2023/11/14 22:13".into())
        );
        assert_eq!(
            eval_unixepoch(&[Value::Text("2026-11-16 00:00:00".into())]).unwrap(),
            Value::Integer(1_794_787_200)
        );
    }
}
