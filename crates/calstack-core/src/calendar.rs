//! ICS normalization into the strip's local, single-day event model.
use crate::{config::Feed, meeting::recognized_url, Event};
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, TimeZone};
use ical::{parser::ical::component::IcalEvent, property::Property, IcalParser};
use rrule::{RRule, RRuleSet, Tz, Unvalidated};
use std::collections::{BTreeMap, HashSet};

fn prop<'a>(event: &'a IcalEvent, name: &str) -> Option<&'a Property> {
    event.properties.iter().find(|p| p.name == name)
}
fn value<'a>(event: &'a IcalEvent, name: &str) -> Option<&'a str> {
    prop(event, name)?.value.as_deref()
}
fn param<'a>(property: &'a Property, key: &str) -> Option<&'a str> {
    property
        .params
        .as_ref()?
        .iter()
        .find(|(k, _)| k == key)?
        .1
        .first()
        .map(String::as_str)
}
fn is_date(p: &Property) -> bool {
    param(p, "VALUE") == Some("DATE") || p.value.as_ref().is_some_and(|v| v.len() == 8)
}
fn datetime(p: &Property, local: Tz) -> Result<DateTime<Tz>> {
    let raw = p.value.as_deref().context("missing date value")?;
    let zone = if raw.ends_with('Z') {
        Tz::UTC
    } else if let Some(name) = param(p, "TZID") {
        name.trim_matches('"')
            .parse::<chrono_tz::Tz>()
            .map(Tz::from)
            .map_err(|_| anyhow::anyhow!("unsupported timezone identifier"))?
    } else {
        local
    };
    let naive = if is_date(p) {
        NaiveDate::parse_from_str(raw, "%Y%m%d")?
            .and_hms_opt(0, 0, 0)
            .unwrap()
    } else {
        NaiveDateTime::parse_from_str(raw.trim_end_matches('Z'), "%Y%m%dT%H%M%S")?
    };
    zone.from_local_datetime(&naive)
        .earliest()
        .context("date falls in a timezone clock gap")
}
fn duration(raw: &str) -> Result<Duration> {
    let raw = raw.strip_prefix('+').unwrap_or(raw);
    let body = raw.strip_prefix('P').context("invalid event duration")?;
    let mut seconds = 0_i64;
    let mut number = String::new();
    let mut time = false;
    let mut fields = 0;
    for ch in body.chars() {
        if ch.is_ascii_digit() {
            number.push(ch);
            continue;
        }
        if ch == 'T' && number.is_empty() && !time {
            time = true;
            continue;
        }
        let multiplier = match (time, ch) {
            (false, 'W') => 604800,
            (false, 'D') => 86400,
            (true, 'H') => 3600,
            (true, 'M') => 60,
            (true, 'S') => 1,
            _ => bail!("invalid event duration"),
        };
        let n = number.parse::<i64>()?;
        seconds = seconds
            .checked_add(n.checked_mul(multiplier).context("duration too large")?)
            .context("duration too large")?;
        fields += 1;
        number.clear();
    }
    if !number.is_empty() || fields == 0 {
        bail!("invalid event duration");
    }
    Duration::try_seconds(seconds).context("duration too large")
}
fn unescape(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n' | 'N') => out.push('\n'),
                Some(c) => out.push(c),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}
fn meeting(event: &IcalEvent) -> Option<String> {
    for field in ["URL", "LOCATION", "DESCRIPTION"] {
        let text = unescape(value(event, field).unwrap_or_default());
        for (offset, _) in text.match_indices("http") {
            let candidate = text[offset..]
                .split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"'))
                .next()?;
            let candidate = candidate.trim_end_matches(['.', ',', ';', ')', ']', '}']);
            if let Some(url) = recognized_url(candidate) {
                return Some(url.into());
            }
        }
    }
    None
}
fn cancelled(event: &IcalEvent) -> bool {
    value(event, "STATUS") == Some("CANCELLED")
}
fn event_duration(event: &IcalEvent, start: DateTime<Tz>, local: Tz) -> Result<Duration> {
    let result = if let Some(end) = prop(event, "DTEND") {
        datetime(end, local)? - start
    } else if let Some(raw) = value(event, "DURATION") {
        duration(raw)?
    } else {
        Duration::zero()
    };
    if result < Duration::zero() || result > Duration::days(366) {
        bail!("event duration must be between zero and 366 days");
    }
    Ok(result)
}

/// Expand only occurrences intersecting `day`. Floating times use `local`.
/// IANA timezones are supported; custom VTIMEZONE rules and THISANDFUTURE are rejected.
pub fn events_for_day(source: &str, feed: &Feed, day: NaiveDate, local: Tz) -> Result<Vec<Event>> {
    let midnight = day.and_hms_opt(0, 0, 0).unwrap();
    let day_start = local
        .from_local_datetime(&midnight)
        .earliest()
        .context("invalid local midnight")?;
    let next_midnight = day
        .succ_opt()
        .context("date overflow")?
        .and_hms_opt(0, 0, 0)
        .unwrap();
    let day_end = local
        .from_local_datetime(&next_midnight)
        .earliest()
        .context("invalid local midnight")?;
    let mut calendars = 0;
    let mut result = Vec::new();
    for calendar in IcalParser::new(source.as_bytes()) {
        let calendar = calendar.context("invalid iCalendar document")?;
        calendars += 1;
        // Keep the newest revision of each UID/RECURRENCE-ID pair.
        let mut revisions: BTreeMap<(String, Option<i64>), IcalEvent> = BTreeMap::new();
        for event in calendar.events {
            let uid = value(&event, "UID")
                .context("event missing UID")?
                .to_owned();
            let recurrence = prop(&event, "RECURRENCE-ID")
                .map(|p| {
                    if param(p, "RANGE").is_some() {
                        bail!("recurrence RANGE overrides are not supported");
                    }
                    Ok(datetime(p, local)?.timestamp())
                })
                .transpose()?;
            let key = (uid, recurrence);
            let rank = |e: &IcalEvent| {
                (
                    value(e, "SEQUENCE")
                        .and_then(|v| v.parse::<u64>().ok())
                        .unwrap_or(0),
                    value(e, "DTSTAMP").unwrap_or("").to_owned(),
                )
            };
            if revisions
                .get(&key)
                .is_none_or(|old| rank(&event) >= rank(old))
            {
                revisions.insert(key, event);
            }
        }
        let mut overrides = HashSet::new();
        for (uid, rid) in revisions.keys() {
            if let Some(rid) = rid {
                overrides.insert((uid.clone(), *rid));
            }
        }
        for ((uid, rid), raw) in &revisions {
            if cancelled(raw) {
                continue;
            }
            let mut event = raw.clone();
            if rid.is_some() {
                if let Some(master) = revisions.get(&(uid.clone(), None)) {
                    if cancelled(master) {
                        continue;
                    }
                    for name in ["SUMMARY", "LOCATION", "DESCRIPTION", "URL"] {
                        if prop(&event, name).is_none() {
                            if let Some(p) = prop(master, name) {
                                event.properties.push(p.clone());
                            }
                        }
                    }
                    if prop(&event, "DTEND").is_none() && prop(&event, "DURATION").is_none() {
                        if let Some(start) = prop(master, "DTSTART") {
                            let seconds = event_duration(master, datetime(start, local)?, local)?
                                .num_seconds();
                            event.properties.push(Property {
                                name: "DURATION".into(),
                                params: None,
                                value: Some(format!("PT{seconds}S")),
                            });
                        }
                    }
                }
            }
            let start_prop = prop(&event, "DTSTART").context("event missing DTSTART")?;
            // All-day entries intentionally do not occupy the timed strip.
            if is_date(start_prop) {
                continue;
            }
            let start = datetime(start_prop, local)?;
            let span = event_duration(&event, start, local)?;
            let mut occurrences = vec![start];
            if rid.is_none() {
                let mut set = RRuleSet::new(start).rdate(start);
                for p in &event.properties {
                    match p.name.as_str() {
                        "RRULE" => {
                            let rule: RRule<Unvalidated> =
                                p.value.as_deref().context("empty RRULE")?.parse()?;
                            set = set.rrule(rule.validate(start)?);
                        }
                        "RDATE" | "EXDATE" => {
                            for date in p
                                .value
                                .as_deref()
                                .context("empty recurrence date")?
                                .split(',')
                            {
                                let mut single = p.clone();
                                single.value = Some(date.into());
                                let date = datetime(&single, local)?;
                                set = if p.name == "RDATE" {
                                    set.rdate(date)
                                } else {
                                    set.exdate(date)
                                };
                            }
                        }
                        "EXRULE" => bail!("EXRULE is unsupported; use EXDATE"),
                        _ => {}
                    }
                }
                let expanded = set.after(day_start - span).before(day_end).all(4096);
                if expanded.limited {
                    bail!("recurrence expansion exceeded safety limit");
                }
                occurrences = expanded.dates;
            }
            // DTSTART can also be generated by RRULE or repeated in RDATE.
            occurrences.sort_by_key(DateTime::timestamp);
            occurrences.dedup_by_key(|date| date.timestamp());
            for occurrence in occurrences {
                if rid.is_none() && overrides.contains(&(uid.clone(), occurrence.timestamp())) {
                    continue;
                }
                let end = occurrence
                    .checked_add_signed(span)
                    .context("event end overflow")?;
                if occurrence >= day_end || end <= day_start {
                    continue;
                }
                let local_start = occurrence.with_timezone(&local).naive_local();
                let local_end = end.with_timezone(&local).naive_local();
                let start_min = (local_start - midnight).num_seconds().div_euclid(60) as i32;
                let end_min = ((local_end - midnight).num_seconds() + 59).div_euclid(60) as i32;
                result.push(Event {
                    title: unescape(value(&event, "SUMMARY").unwrap_or("Untitled event")),
                    calendar: feed.name.clone(),
                    start: start_min,
                    end: end_min.max(start_min + 1),
                    meeting: meeting(&event),
                    color: feed.color_rgb()?,
                });
            }
        }
    }
    if calendars == 0 {
        bail!("response contains no iCalendar calendar");
    }
    result.sort_by_key(|e| (e.start, e.end));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn feed() -> Feed {
        Feed {
            name: "Work".into(),
            url: None,
            path: Some("test.ics".into()),
            color: Some("#123456".into()),
            enabled: true,
        }
    }
    fn parse(events: &str, date: &str, tz: Tz) -> Vec<Event> {
        events_for_day(
            &format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{events}\r\nEND:VCALENDAR\r\n"),
            &feed(),
            date.parse().unwrap(),
            tz,
        )
        .unwrap()
    }
    #[test]
    fn recurrence_exclusions_overrides_and_links() {
        let source = "BEGIN:VEVENT\nUID:a\nDTSTART:20260920T090000Z\nDURATION:PT1H\nRRULE:FREQ=DAILY;COUNT=10\nEXDATE:20260925T090000Z\nSUMMARY:Standup\\, team\nDESCRIPTION:Join https://meet.google.com/abc-\n defg-hij\nEND:VEVENT\nBEGIN:VEVENT\nUID:a\nRECURRENCE-ID:20260926T090000Z\nDTSTART:20260926T110000Z\nEND:VEVENT";
        assert!(parse(source, "2026-09-25", Tz::UTC).is_empty());
        let events = parse(source, "2026-09-26", Tz::UTC);
        assert_eq!(events.len(), 1);
        assert_eq!((events[0].start, events[0].end), (660, 720));
        assert_eq!(events[0].title, "Standup, team");
        assert_eq!(
            events[0].meeting.as_deref(),
            Some("https://meet.google.com/abc-defg-hij")
        );
        assert_eq!(events[0].color, Some([0x12, 0x34, 0x56]));
    }
    #[test]
    fn timezone_recurrence_tracks_dst() {
        let source = "BEGIN:VEVENT\nUID:a\nDTSTART;TZID=America/New_York:20260307T090000\nDURATION:PT1H\nRRULE:FREQ=DAILY;COUNT=3\nEND:VEVENT";
        assert_eq!(parse(source, "2026-03-07", Tz::UTC)[0].start, 14 * 60);
        assert_eq!(parse(source, "2026-03-08", Tz::UTC)[0].start, 13 * 60);
    }
    #[test]
    fn midnight_floating_dates_cancellation_and_rdate() {
        let source = "BEGIN:VEVENT\nUID:a\nDTSTART:20260925T233000\nDTEND:20260926T003000\nRDATE:20260927T233000\nEND:VEVENT\nBEGIN:VEVENT\nUID:b\nDTSTART;VALUE=DATE:20260926\nEND:VEVENT\nBEGIN:VEVENT\nUID:c\nDTSTART:20260926T090000Z\nSTATUS:CANCELLED\nEND:VEVENT";
        let events = parse(source, "2026-09-26", Tz::Asia__Tokyo);
        assert_eq!(events.len(), 1);
        assert_eq!((events[0].start, events[0].end), (-30, 30));
        assert_eq!(parse(source, "2026-09-28", Tz::Asia__Tokyo).len(), 1);
    }
    #[test]
    fn cancelled_override_suppresses_occurrence() {
        let source = "BEGIN:VEVENT\nUID:a\nDTSTART:20260920T090000Z\nDURATION:PT1H\nRRULE:FREQ=DAILY\nEND:VEVENT\nBEGIN:VEVENT\nUID:a\nRECURRENCE-ID:20260926T090000Z\nSTATUS:CANCELLED\nEND:VEVENT";
        assert!(parse(source, "2026-09-26", Tz::UTC).is_empty());
    }
    #[test]
    fn rejects_html_and_unknown_timezone() {
        assert!(events_for_day(
            "<html>login</html>",
            &feed(),
            "2026-09-26".parse().unwrap(),
            Tz::UTC
        )
        .is_err());
        let source = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:a\nDTSTART;TZID=Custom:20260926T090000\nEND:VEVENT\nEND:VCALENDAR";
        assert!(events_for_day(source, &feed(), "2026-09-26".parse().unwrap(), Tz::UTC).is_err());
    }

    #[test]
    fn fixture_has_no_duplicate_first_recurrence() {
        let events = events_for_day(
            include_str!("../../../assets/demo.ics"),
            &feed(),
            "2026-09-26".parse().unwrap(),
            Tz::Asia__Tokyo,
        )
        .unwrap();
        assert_eq!(events.len(), 10);
        assert_eq!(events.iter().filter(|event| event.start == -30).count(), 1);
    }
}
