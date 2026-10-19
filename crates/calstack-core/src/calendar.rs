//! Normalize ICS recurrence sets into the strip's local, single-day event model.
use crate::{
    config::Feed,
    meeting::recognized_url,
    zones::{self, date_only, parameter, stamp, Stamp, Zone, Zones},
    Event,
};
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use ical::{parser::ical::component::IcalEvent, property::Property, IcalParser};
use rrule::{RRuleSet, Tz};
use std::collections::{BTreeMap, HashMap, HashSet};

fn prop<'a>(event: &'a IcalEvent, name: &str) -> Option<&'a Property> {
    event.properties.iter().find(|p| p.name == name)
}
fn value<'a>(event: &'a IcalEvent, name: &str) -> Option<&'a str> {
    prop(event, name)?.value.as_deref()
}
fn cancelled(event: &IcalEvent) -> bool {
    value(event, "STATUS") == Some("CANCELLED")
}
fn address(raw: &str) -> String {
    let raw = raw.trim();
    let raw = if raw.len() >= 7 && raw[..7].eq_ignore_ascii_case("mailto:") {
        &raw[7..]
    } else {
        raw
    };
    raw.to_ascii_lowercase()
}
/// The feed owner is invited to everything on their calendar, so take the one
/// address listed on at least 90% of events that have attendees. Small or
/// ambiguous feeds yield nobody rather than a guess.
fn owner(events: &[IcalEvent]) -> HashSet<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut invited = 0;
    for event in events {
        let addresses: HashSet<String> = event
            .properties
            .iter()
            .filter(|p| p.name == "ATTENDEE")
            .filter_map(|p| p.value.as_deref())
            .map(address)
            .collect();
        if addresses.is_empty() {
            continue;
        }
        invited += 1;
        for a in addresses {
            *counts.entry(a).or_default() += 1;
        }
    }
    let likely: HashSet<String> = counts
        .into_iter()
        .filter(|(_, n)| n * 10 >= invited * 9)
        .map(|(a, _)| a)
        .collect();
    if invited >= 5 && likely.len() == 1 {
        likely
    } else {
        HashSet::new()
    }
}
/// Whether one of `me` declined. Overrides that list no attendees inherit the master's.
fn declined(event: &IcalEvent, master: Option<&IcalEvent>, me: &HashSet<String>) -> bool {
    let attendees = |e: &IcalEvent| {
        e.properties
            .iter()
            .filter(|p| p.name == "ATTENDEE")
            .cloned()
            .collect::<Vec<_>>()
    };
    let mut list = attendees(event);
    if list.is_empty() {
        list = master.map(attendees).unwrap_or_default();
    }
    list.iter().any(|p| {
        parameter(p, "PARTSTAT").is_some_and(|s| s.eq_ignore_ascii_case("DECLINED"))
            && p.value.as_deref().is_some_and(|v| me.contains(&address(v)))
    })
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
            if let Some(url) =
                recognized_url(candidate.trim_end_matches(['.', ',', ';', ')', ']', '}']))
            {
                return Some(url.into());
            }
        }
    }
    None
}
#[derive(Clone, Copy, Default)]
struct Span {
    days: i64,
    seconds: i64,
}
impl Span {
    fn bound(self) -> Duration {
        Duration::seconds(self.days * 86400 + self.seconds)
    }
    fn checked(self) -> Result<Self> {
        if self.days < 0
            || self.seconds < 0
            || self.days > 366
            || self.seconds > 366 * 86400
            || self.bound() > Duration::days(366)
        {
            bail!("event duration must be between zero and 366 days");
        }
        Ok(self)
    }
    fn end(self, start: DateTime<Utc>, zone: &Zone) -> Result<DateTime<Utc>> {
        let start = if self.days == 0 {
            start
        } else {
            zone.instant(zone.wall(start) + Duration::days(self.days), false)?
                .context("invalid event end")?
        };
        start
            .checked_add_signed(Duration::seconds(self.seconds))
            .context("event end overflow")
    }
}
fn duration(raw: &str) -> Result<Span> {
    let body = raw
        .strip_prefix('+')
        .unwrap_or(raw)
        .strip_prefix('P')
        .context("invalid event duration")?;
    let mut span = Span::default();
    let mut number = String::new();
    let mut time = false;
    let mut rank = 0;
    if body.is_empty() || body.ends_with('T') {
        bail!("invalid event duration");
    }
    for ch in body.chars() {
        if ch.is_ascii_digit() {
            number.push(ch);
            continue;
        }
        if ch == 'T' && number.is_empty() && !time {
            time = true;
            continue;
        }
        let (order, multiplier, days) = match (time, ch) {
            (false, 'W') if rank == 0 && body.ends_with('W') => (1, 7, true),
            (false, 'D') => (2, 1, true),
            (true, 'H') => (3, 3600, false),
            (true, 'M') => (4, 60, false),
            (true, 'S') => (5, 1, false),
            _ => bail!("invalid event duration"),
        };
        if order <= rank {
            bail!("invalid event duration");
        }
        rank = order;
        let amount = number
            .parse::<i64>()?
            .checked_mul(multiplier)
            .context("duration overflow")?;
        if days {
            span.days = span.days.checked_add(amount).context("duration overflow")?;
        } else {
            span.seconds = span
                .seconds
                .checked_add(amount)
                .context("duration overflow")?;
        }
        number.clear();
    }
    if !number.is_empty() || rank == 0 {
        bail!("invalid event duration");
    }
    span.checked()
}
fn event_span(
    event: &IcalEvent,
    start: &Stamp,
    local: Tz,
    zones: &Zones,
    fallback: Span,
) -> Result<Span> {
    if let Some(end) = prop(event, "DTEND") {
        Span {
            days: 0,
            seconds: (stamp(end, local, zones)?.instant()? - start.instant()?).num_seconds(),
        }
        .checked()
    } else if let Some(raw) = value(event, "DURATION") {
        duration(raw)
    } else {
        Ok(fallback)
    }
}
fn prepared(
    raw: &IcalEvent,
    master: Option<&IcalEvent>,
    local: Tz,
    zones: &Zones,
) -> Result<(IcalEvent, Stamp, Span)> {
    let mut event = raw.clone();
    let mut fallback = Span::default();
    if let Some(master) = master {
        for name in ["SUMMARY", "DESCRIPTION", "LOCATION", "URL"] {
            if prop(&event, name).is_none() {
                if let Some(p) = prop(master, name) {
                    event.properties.push(p.clone());
                }
            }
        }
        let start = stamp(
            prop(master, "DTSTART").context("master missing DTSTART")?,
            local,
            zones,
        )?;
        fallback = event_span(master, &start, local, zones, fallback)?;
    }
    let start = stamp(
        prop(&event, "DTSTART").context("event missing DTSTART")?,
        local,
        zones,
    )?;
    let span = event_span(&event, &start, local, zones, fallback)?;
    Ok((event, start, span))
}
struct View<'a> {
    feed: &'a Feed,
    day: NaiveDate,
    local: Tz,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
}
impl View<'_> {
    fn append(
        &self,
        output: &mut Vec<Event>,
        event: &IcalEvent,
        start: DateTime<Utc>,
        span: Span,
        zone: &Zone,
    ) -> Result<()> {
        let end = span.end(start, zone)?;
        if start >= self.end || end < self.start || (end == self.start && start != end) {
            return Ok(());
        }
        let midnight = self.day.and_hms_opt(0, 0, 0).unwrap();
        let start_min = (start.with_timezone(&self.local).naive_local() - midnight)
            .num_seconds()
            .div_euclid(60) as i32;
        let end_min = ((end.with_timezone(&self.local).naive_local() - midnight).num_seconds() + 59)
            .div_euclid(60) as i32;
        output.push(Event {
            title: unescape(value(event, "SUMMARY").unwrap_or("Untitled event")),
            calendar: self.feed.name.clone(),
            start: start_min,
            end: end_min.max(start_min + 1),
            meeting: meeting(event),
            color: self.feed.color_rgb()?,
        });
        Ok(())
    }
}

/// Expand only occurrences intersecting `day`; floating timestamps use `local`.
pub fn events_for_day(source: &str, feed: &Feed, day: NaiveDate, local: Tz) -> Result<Vec<Event>> {
    let start = local
        .from_local_datetime(&day.and_hms_opt(0, 0, 0).unwrap())
        .earliest()
        .context("invalid local midnight")?
        .with_timezone(&Utc);
    let end = local
        .from_local_datetime(
            &day.succ_opt()
                .context("date overflow")?
                .and_hms_opt(0, 0, 0)
                .unwrap(),
        )
        .earliest()
        .context("invalid local midnight")?
        .with_timezone(&Utc);
    let view = View {
        feed,
        day,
        local,
        start,
        end,
    };
    let mut output = Vec::new();
    let mut calendars = 0;
    for calendar in IcalParser::new(source.as_bytes()) {
        let calendar = calendar.context("invalid iCalendar document")?;
        calendars += 1;
        let zones = zones::load(&calendar.timezones, day)?;
        let me = owner(&calendar.events);
        let mut revisions: BTreeMap<(String, Option<i64>), IcalEvent> = BTreeMap::new();
        for event in calendar.events {
            let uid = value(&event, "UID")
                .context("event missing UID")?
                .to_owned();
            let rid = prop(&event, "RECURRENCE-ID")
                .map(|p| -> Result<i64> {
                    if parameter(p, "RANGE").is_some_and(|range| range != "THISANDFUTURE") {
                        bail!("unknown recurrence RANGE");
                    }
                    Ok(stamp(p, local, &zones)?.instant()?.timestamp())
                })
                .transpose()?;
            let rank = |e: &IcalEvent| {
                (
                    value(e, "SEQUENCE")
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(0),
                    value(e, "DTSTAMP").unwrap_or("").to_owned(),
                )
            };
            let key = (uid, rid);
            if revisions
                .get(&key)
                .is_none_or(|old| rank(&event) >= rank(old))
            {
                revisions.insert(key, event);
            }
        }
        let overrides: HashSet<_> = revisions
            .keys()
            .filter_map(|(uid, rid)| rid.map(|rid| (uid.clone(), rid)))
            .collect();
        for ((uid, rid), raw) in &revisions {
            if cancelled(raw) {
                continue;
            }
            let master = rid.and_then(|_| revisions.get(&(uid.clone(), None)));
            if master.is_some_and(cancelled) || declined(raw, master, &me) {
                continue;
            }
            if prop(raw, "DTSTART").is_some_and(date_only) {
                continue;
            }
            let (event, start, span) = prepared(raw, master, local, &zones)?;
            if rid.is_some() {
                view.append(&mut output, &event, start.instant()?, span, &start.zone)?;
                continue;
            }
            let mut ranges = Vec::new();
            let mut max_shift = Duration::zero();
            let mut max_span = span.bound();
            for ((other_uid, other_rid), other) in &revisions {
                if other_uid != uid {
                    continue;
                }
                if let Some(id) = other_rid.filter(|_| {
                    prop(other, "RECURRENCE-ID").and_then(|p| parameter(p, "RANGE"))
                        == Some("THISANDFUTURE")
                }) {
                    if cancelled(other) || declined(other, Some(raw), &me) {
                        ranges.push((id, None));
                        continue;
                    }
                    let (changed, changed_start, changed_span) =
                        prepared(other, Some(raw), local, &zones)?;
                    let identity =
                        DateTime::from_timestamp(id, 0).context("invalid recurrence identity")?;
                    let shift =
                        start.zone.wall(changed_start.instant()?) - start.zone.wall(identity);
                    if shift.abs() > Duration::days(366) {
                        bail!("recurrence shift exceeds safety limit");
                    }
                    max_shift = max_shift.max(shift.abs());
                    max_span = max_span.max(changed_span.bound());
                    ranges.push((id, Some((changed, changed_span, shift))));
                }
            }
            let recurrence_start = start.recurrence_start()?;
            let mut set =
                RRuleSet::new(recurrence_start).rdate(start.zone.recurrence_date(start.instant()?));
            let mut periods: HashMap<i64, Span> = HashMap::new();
            for p in &event.properties {
                match p.name.as_str() {
                    "RRULE" | "EXRULE" => {
                        let raw = p.value.as_deref().context("empty recurrence rule")?;
                        if let Some(dates) = start.zone.counted_dates(
                            raw,
                            recurrence_start,
                            view.end + max_shift + Duration::days(2),
                        )? {
                            for date in dates {
                                set = if p.name == "RRULE" {
                                    set.rdate(date)
                                } else {
                                    set.exdate(date)
                                };
                            }
                            continue;
                        }
                        let rule = start.zone.rule(raw, recurrence_start)?;
                        set = if p.name == "RRULE" {
                            set.rrule(rule)
                        } else {
                            set.exrule(rule)
                        };
                    }
                    "RDATE" | "EXDATE" => {
                        for date in p
                            .value
                            .as_deref()
                            .context("empty recurrence date")?
                            .split(',')
                        {
                            let (date, end) = date
                                .split_once('/')
                                .map_or((date, None), |(a, b)| (a, Some(b)));
                            let mut property = p.clone();
                            property.value = Some(date.into());
                            let date = stamp(&property, local, &zones)?;
                            let instant = date.instant()?;
                            if let Some(end) = end {
                                if p.name != "RDATE" {
                                    bail!("periods are only valid in RDATE");
                                }
                                let period_span = if end.starts_with(['P', '+']) {
                                    duration(end)?
                                } else {
                                    property.value = Some(end.into());
                                    Span {
                                        days: 0,
                                        seconds: (stamp(&property, local, &zones)?.instant()?
                                            - instant)
                                            .num_seconds(),
                                    }
                                    .checked()?
                                };
                                max_span = max_span.max(period_span.bound());
                                periods.insert(instant.timestamp(), period_span);
                            }
                            let date = start.zone.recurrence_date(instant);
                            set = if p.name == "RDATE" {
                                set.rdate(date)
                            } else {
                                set.exdate(date)
                            };
                        }
                    }
                    _ => {}
                }
            }
            // Two days cover timezone/DST differences for nominal-day durations.
            let expanded = set
                .after(
                    start
                        .zone
                        .recurrence_date(view.start - max_span - max_shift - Duration::days(2)),
                )
                .before(
                    start
                        .zone
                        .recurrence_date(view.end + max_shift + Duration::days(2)),
                )
                .all(16384);
            if expanded.limited {
                bail!("recurrence expansion exceeded safety limit");
            }
            let mut seen = HashSet::new();
            for date in expanded.dates {
                let Some(instant) = start.zone.occurrence(date)? else {
                    continue;
                };
                if !seen.insert(instant.timestamp())
                    || overrides.contains(&(uid.clone(), instant.timestamp()))
                {
                    continue;
                }
                if let Some((_, change)) = ranges
                    .iter()
                    .rev()
                    .find(|(id, _)| *id <= instant.timestamp())
                {
                    if let Some((changed, span, shift)) = change {
                        if prop(changed, "DTSTART").is_some_and(date_only) {
                            continue;
                        }
                        if let Some(moved) = start
                            .zone
                            .instant(start.zone.wall(instant) + *shift, true)?
                        {
                            view.append(&mut output, changed, moved, *span, &start.zone)?;
                        }
                    }
                } else {
                    view.append(
                        &mut output,
                        &event,
                        instant,
                        periods.get(&instant.timestamp()).copied().unwrap_or(span),
                        &start.zone,
                    )?;
                }
            }
        }
    }
    if calendars == 0 {
        bail!("response contains no iCalendar calendar");
    }
    output.sort_by_key(|event| (event.start, event.end));
    Ok(output)
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
    fn named_titles(source: &str, name: &str) -> Vec<String> {
        events_for_day(
            &format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nX-WR-CALNAME:{name}\r\n{source}\r\nEND:VCALENDAR\r\n"),
            &feed(),
            "2026-09-26".parse().unwrap(),
            Tz::UTC,
        )
        .unwrap()
        .into_iter()
        .map(|e| e.title)
        .collect()
    }
    /// Earlier meetings that make me@example.com recognizable as the feed owner.
    fn titles(source: &str) -> Vec<String> {
        let past: String = (20..25)
            .map(|d| format!("BEGIN:VEVENT\nUID:p{d}\nDTSTART:202609{d}T090000Z\nDURATION:PT1H\nATTENDEE;PARTSTAT=ACCEPTED:mailto:me@example.com\nATTENDEE;PARTSTAT=ACCEPTED:mailto:them{}@example.com\nEND:VEVENT\n", d % 2))
            .collect();
        named_titles(&format!("{past}{source}"), "Work")
    }
    #[test]
    fn hides_only_events_you_declined() {
        let source = "BEGIN:VEVENT\nUID:a\nSUMMARY:Mine\nDTSTART:20260926T090000Z\nDURATION:PT1H\nATTENDEE;PARTSTAT=DECLINED;CN=Me:MAILTO:Me@Example.com\nEND:VEVENT\nBEGIN:VEVENT\nUID:b\nSUMMARY:Theirs\nDTSTART:20260926T100000Z\nDURATION:PT1H\nATTENDEE;PARTSTAT=ACCEPTED:mailto:me@example.com\nATTENDEE;PARTSTAT=DECLINED:mailto:them0@example.com\nEND:VEVENT";
        assert_eq!(titles(source), ["Theirs"]);
    }
    #[test]
    fn declines_apply_to_series_and_single_occurrences() {
        let declined_series = "BEGIN:VEVENT\nUID:a\nSUMMARY:Series\nDTSTART:20260920T090000Z\nDURATION:PT1H\nRRULE:FREQ=DAILY\nATTENDEE;PARTSTAT=DECLINED:mailto:me@example.com\nEND:VEVENT\nBEGIN:VEVENT\nUID:a\nRECURRENCE-ID:20260926T090000Z\nSUMMARY:Moved\nDTSTART:20260926T120000Z\nDURATION:PT1H\nEND:VEVENT";
        assert!(titles(declined_series).is_empty());
        let rejoined = declined_series.replace(
            "SUMMARY:Moved\n",
            "SUMMARY:Moved\nATTENDEE;PARTSTAT=ACCEPTED:mailto:me@example.com\n",
        );
        assert_eq!(titles(&rejoined), ["Moved"]);
        let one_off = "BEGIN:VEVENT\nUID:a\nSUMMARY:Series\nDTSTART:20260920T090000Z\nDURATION:PT1H\nRRULE:FREQ=DAILY\nATTENDEE;PARTSTAT=ACCEPTED:mailto:me@example.com\nEND:VEVENT\nBEGIN:VEVENT\nUID:a\nRECURRENCE-ID:20260926T090000Z\nDTSTART:20260926T090000Z\nDURATION:PT1H\nATTENDEE;PARTSTAT=DECLINED:mailto:me@example.com\nEND:VEVENT";
        assert!(titles(one_off).is_empty());
        assert_eq!(titles(&one_off.replace("20260926", "20260925")), ["Series"]);
    }
    #[test]
    fn owner_is_never_guessed_from_too_little_or_the_calendar_name() {
        let today = "BEGIN:VEVENT\nUID:t\nSUMMARY:Declined\nDTSTART:20260926T090000Z\nDURATION:PT1H\nATTENDEE;PARTSTAT=DECLINED:mailto:me@example.com\nATTENDEE;PARTSTAT=ACCEPTED:mailto:them0@example.com\nEND:VEVENT";
        assert_eq!(named_titles(today, "Work"), ["Declined"]);
        assert_eq!(named_titles(today, "me@example.com"), ["Declined"]);
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
    #[test]
    fn calendar_supplied_timezone_tracks_dst() {
        let source = include_str!("../../../assets/fixtures/custom-timezone.ics");
        for (day, hour) in [("2026-03-07", 14), ("2026-03-08", 13), ("2026-03-09", 13)] {
            let events = events_for_day(source, &feed(), day.parse().unwrap(), Tz::UTC).unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].start, hour * 60);
        }
    }

    #[test]
    fn custom_timezone_gap_does_not_consume_recurrence_count() {
        let source = include_str!("../../../assets/fixtures/custom-timezone.ics")
            .replace("20260307T090000", "20260307T023000");
        assert!(
            events_for_day(&source, &feed(), "2026-03-08".parse().unwrap(), Tz::UTC)
                .unwrap()
                .is_empty()
        );
        let events =
            events_for_day(&source, &feed(), "2026-03-10".parse().unwrap(), Tz::UTC).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].start, 6 * 60 + 30);
    }
    #[test]
    fn windows_timezone_names_are_mapped() {
        let source = "BEGIN:VEVENT\nUID:windows\nDTSTART;TZID=Eastern Standard Time:20260308T090000\nDURATION:PT1H\nEND:VEVENT";
        assert_eq!(parse(source, "2026-03-08", Tz::UTC)[0].start, 13 * 60);
    }
    #[test]
    fn this_and_future_moves_changes_duration_and_cancels() {
        let source = include_str!("../../../assets/fixtures/range-overrides.ics");
        for day in ["2026-09-25", "2026-09-26"] {
            let events = events_for_day(source, &feed(), day.parse().unwrap(), Tz::UTC).unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!((events[0].start, events[0].end), (660, 780));
            assert_eq!(events[0].title, "Moved series");
        }
        assert!(
            events_for_day(source, &feed(), "2026-09-27".parse().unwrap(), Tz::UTC)
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn rdate_periods_override_duration_and_exrule_excludes() {
        let events = events_for_day(
            include_str!("../../../assets/fixtures/periods-and-exrule.ics"),
            &feed(),
            "2026-09-26".parse().unwrap(),
            Tz::UTC,
        )
        .unwrap();
        assert_eq!(
            events.iter().map(|e| (e.start, e.end)).collect::<Vec<_>>(),
            vec![(720, 840), (960, 990)]
        );
    }
    #[test]
    fn nominal_day_duration_crosses_dst_in_local_time() {
        let source = "BEGIN:VEVENT\nUID:nominal\nDTSTART;TZID=America/New_York:20260307T120000\nDURATION:P1D\nEND:VEVENT";
        assert_eq!(parse(source, "2026-03-08", Tz::UTC)[0].end, 16 * 60);
        assert_eq!(
            parse(&source.replace("P1D", "PT24H"), "2026-03-08", Tz::UTC)[0].end,
            17 * 60
        );
    }
    #[test]
    fn url_location_and_description_identify_meeting_providers() {
        for (field, url) in [
            ("URL", "https://meet.google.com/abc-defg-hij"),
            ("LOCATION", "https://us02web.zoom.us/j/123456"),
            (
                "DESCRIPTION",
                "https://teams.microsoft.com/l/meetup-join/example",
            ),
        ] {
            let source = format!("BEGIN:VEVENT\nUID:meeting\nDTSTART:20260926T090000Z\nDURATION:PT1H\n{field}:{url}\nEND:VEVENT");
            assert_eq!(
                parse(&source, "2026-09-26", Tz::UTC)[0].meeting.as_deref(),
                Some(url)
            );
        }
    }
    #[test]
    fn latest_revision_replaces_prior_event() {
        let source = "BEGIN:VEVENT\nUID:a\nDTSTART:20260926T090000Z\nDURATION:PT1H\nSEQUENCE:2\nSUMMARY:New\nEND:VEVENT\nBEGIN:VEVENT\nUID:a\nDTSTART:20260926T080000Z\nDURATION:PT1H\nSEQUENCE:1\nSUMMARY:Old\nEND:VEVENT";
        let events = parse(source, "2026-09-26", Tz::UTC);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].title, "New");
    }
}
