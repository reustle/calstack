//! Resolve calendar-supplied timezone observances without inventing recurrence rules.
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, Offset, TimeZone, Utc};
use ical::{parser::ical::component::IcalTimeZone, property::Property};
use rrule::{RRule, RRuleSet, Tz, Unvalidated};
use std::{collections::HashMap, sync::Arc};

#[derive(Clone)]
pub(crate) enum Zone {
    Standard(Tz),
    Custom(Arc<CustomZone>),
}
#[derive(Clone)]
pub(crate) struct Stamp {
    pub wall: NaiveDateTime,
    pub zone: Zone,
}
#[derive(Clone)]
struct Transition {
    instant: DateTime<Utc>,
    from: i32,
    to: i32,
}
pub(crate) struct CustomZone {
    transitions: Vec<Transition>,
    offsets: Vec<i32>,
}
pub(crate) type Zones = HashMap<String, Zone>;

pub(crate) fn parameter<'a>(p: &'a Property, name: &str) -> Option<&'a str> {
    p.params
        .as_ref()?
        .iter()
        .find(|(key, _)| key == name)?
        .1
        .first()
        .map(String::as_str)
}
pub(crate) fn date_only(p: &Property) -> bool {
    parameter(p, "VALUE") == Some("DATE") || p.value.as_ref().is_some_and(|value| value.len() == 8)
}
pub(crate) fn naive(raw: &str) -> Result<NaiveDateTime> {
    if raw.len() == 8 {
        Ok(NaiveDate::parse_from_str(raw, "%Y%m%d")?
            .and_hms_opt(0, 0, 0)
            .unwrap())
    } else {
        Ok(NaiveDateTime::parse_from_str(
            raw.trim_end_matches('Z'),
            "%Y%m%dT%H%M%S",
        )?)
    }
}
pub(crate) fn stamp(p: &Property, local: Tz, zones: &Zones) -> Result<Stamp> {
    let value = p.value.as_deref().context("missing date value")?;
    let zone = if value.ends_with('Z') {
        Zone::Standard(Tz::UTC)
    } else if let Some(name) = parameter(p, "TZID") {
        let name = name.trim_matches('"');
        if let Some(zone) = zones.get(name) {
            zone.clone()
        } else {
            let name = name
                .split("/Tzfile/")
                .last()
                .unwrap_or(name)
                .trim_start_matches('/');
            let iana = name.parse::<chrono_tz::Tz>().ok().or_else(|| {
                name.parse::<windows_timezones::WindowsTimezone>()
                    .ok()?
                    .tzdb_id()
                    .parse()
                    .ok()
            });
            Zone::Standard(iana.context("unknown timezone identifier")?.into())
        }
    } else {
        Zone::Standard(local)
    };
    Ok(Stamp {
        wall: naive(value)?,
        zone,
    })
}
impl Stamp {
    pub fn instant(&self) -> Result<DateTime<Utc>> {
        self.zone
            .instant(self.wall, false)?
            .context("invalid local time")
    }
    pub fn recurrence_start(&self) -> Result<DateTime<Tz>> {
        match self.zone {
            Zone::Standard(zone) => Ok(self.instant()?.with_timezone(&zone)),
            Zone::Custom(_) => Ok(Tz::UTC.from_utc_datetime(&self.wall)),
        }
    }
}
impl Zone {
    pub fn wall(&self, instant: DateTime<Utc>) -> NaiveDateTime {
        match self {
            Self::Standard(zone) => instant.with_timezone(zone).naive_local(),
            Self::Custom(zone) => {
                instant.naive_utc() + Duration::seconds(zone.offset(instant) as i64)
            }
        }
    }
    pub fn instant(&self, wall: NaiveDateTime, generated: bool) -> Result<Option<DateTime<Utc>>> {
        match self {
            Self::Standard(zone) => {
                if let Some(date) = zone.from_local_datetime(&wall).earliest() {
                    return Ok(Some(date.with_timezone(&Utc)));
                }
                if generated {
                    return Ok(None);
                }
                // RFC 5545: explicit times in a gap use the offset before it.
                let before = zone
                    .from_local_datetime(&(wall - Duration::hours(3)))
                    .earliest()
                    .context("cannot resolve timezone clock gap")?;
                Ok(Some(
                    before
                        .offset()
                        .fix()
                        .from_local_datetime(&wall)
                        .single()
                        .context("invalid offset")?
                        .with_timezone(&Utc),
                ))
            }
            Self::Custom(zone) => {
                let resolved = zone
                    .offsets
                    .iter()
                    .filter_map(|offset| {
                        let instant = wall.and_utc() - Duration::seconds(*offset as i64);
                        (zone.offset(instant) == *offset).then_some(instant)
                    })
                    .min();
                if resolved.is_some() || generated {
                    return Ok(resolved);
                }
                for transition in &zone.transitions {
                    let before =
                        transition.instant.naive_utc() + Duration::seconds(transition.from as i64);
                    let after =
                        transition.instant.naive_utc() + Duration::seconds(transition.to as i64);
                    if before <= wall && wall < after {
                        return Ok(Some(
                            wall.and_utc() - Duration::seconds(transition.from as i64),
                        ));
                    }
                }
                bail!("cannot resolve calendar timezone")
            }
        }
    }
    pub fn recurrence_date(&self, instant: DateTime<Utc>) -> DateTime<Tz> {
        match self {
            Self::Standard(zone) => instant.with_timezone(zone),
            Self::Custom(_) => Tz::UTC.from_utc_datetime(&self.wall(instant)),
        }
    }
    pub fn occurrence(&self, date: DateTime<Tz>) -> Result<Option<DateTime<Utc>>> {
        match self {
            Self::Standard(_) => Ok(Some(date.with_timezone(&Utc))),
            Self::Custom(_) => self.instant(date.naive_local(), true),
        }
    }
    pub fn rule(&self, raw: &str, start: DateTime<Tz>) -> Result<RRule> {
        let text = if matches!(self, Self::Custom(_)) {
            rewrite_until(raw, |instant| self.wall(instant))?
        } else {
            raw.to_owned()
        };
        Ok(text.parse::<RRule<Unvalidated>>()?.validate(start)?)
    }

    /// Custom zones expand in civil time. Gap instances must not consume COUNT
    /// slots, so count real occurrences before putting them into the set.
    pub fn counted_dates(
        &self,
        raw: &str,
        start: DateTime<Tz>,
        before: DateTime<Utc>,
    ) -> Result<Option<Vec<DateTime<Tz>>>> {
        if !matches!(self, Self::Custom(_)) {
            return Ok(None);
        }
        let Some(count) = self.rule(raw, start)?.get_count() else {
            return Ok(None);
        };
        let uncounted = raw
            .split(';')
            .filter(|part| !part.starts_with("COUNT="))
            .collect::<Vec<_>>()
            .join(";");
        let result = RRuleSet::new(start)
            .rrule(self.rule(&uncounted, start)?)
            .before(self.recurrence_date(before))
            .all(u16::MAX);
        if result.limited {
            bail!("custom timezone recurrence exceeded safety limit");
        }
        let mut dates = Vec::new();
        for date in result.dates {
            if self.occurrence(date)?.is_some() {
                dates.push(date);
            }
            if dates.len() >= count as usize {
                break;
            }
        }
        Ok(Some(dates))
    }
}
impl CustomZone {
    fn offset(&self, instant: DateTime<Utc>) -> i32 {
        let index = self
            .transitions
            .partition_point(|transition| transition.instant <= instant);
        index
            .checked_sub(1)
            .map(|i| self.transitions[i].to)
            .unwrap_or(self.transitions[0].from)
    }
}
fn offset(raw: &str) -> Result<i32> {
    if !matches!(raw.len(), 5 | 7)
        || !matches!(raw.as_bytes()[0], b'+' | b'-')
        || !raw.as_bytes()[1..].iter().all(u8::is_ascii_digit)
    {
        bail!("invalid timezone offset");
    }
    let hour = raw[1..3].parse::<i32>()?;
    let minute = raw[3..5].parse::<i32>()?;
    let second = if raw.len() == 7 {
        raw[5..7].parse::<i32>()?
    } else {
        0
    };
    if hour > 23 || minute > 59 || second > 59 {
        bail!("invalid timezone offset");
    }
    Ok((hour * 3600 + minute * 60 + second) * if raw.starts_with('-') { -1 } else { 1 })
}
fn rewrite_until(raw: &str, convert: impl Fn(DateTime<Utc>) -> NaiveDateTime) -> Result<String> {
    raw.split(';')
        .map(|part| {
            if let Some(until) = part
                .strip_prefix("UNTIL=")
                .filter(|value| value.ends_with('Z'))
            {
                Ok(format!(
                    "UNTIL={}Z",
                    convert(naive(until)?.and_utc()).format("%Y%m%dT%H%M%S")
                ))
            } else {
                Ok(part.to_owned())
            }
        })
        .collect::<Result<Vec<_>>>()
        .map(|parts| parts.join(";"))
}
pub(crate) fn load(timezones: &[IcalTimeZone], day: NaiveDate) -> Result<Zones> {
    let through = Tz::UTC
        .with_ymd_and_hms(day.year() + 3, 1, 1, 0, 0, 0)
        .single()
        .context("timezone date overflow")?;
    let mut zones = Zones::new();
    for tz in timezones {
        let name = tz
            .properties
            .iter()
            .find(|p| p.name == "TZID")
            .and_then(|p| p.value.clone())
            .context("timezone missing TZID")?;
        let mut transitions = Vec::new();
        let mut offsets = Vec::new();
        for transition in &tz.transitions {
            let property = |key: &str| {
                transition
                    .properties
                    .iter()
                    .find(|p| p.name == key)
                    .and_then(|p| p.value.as_deref())
                    .context("timezone observance missing a required property")
            };
            let from = offset(property("TZOFFSETFROM")?)?;
            let to = offset(property("TZOFFSETTO")?)?;
            let start = Tz::UTC.from_utc_datetime(&naive(property("DTSTART")?)?);
            let mut set = RRuleSet::new(start).rdate(start);
            for p in &transition.properties {
                match p.name.as_str() {
                    "RRULE" => {
                        let rule = rewrite_until(
                            p.value.as_deref().context("empty timezone RRULE")?,
                            |instant| instant.naive_utc() + Duration::seconds(from as i64),
                        )?;
                        set = set.rrule(rule.parse::<RRule<Unvalidated>>()?.validate(start)?);
                    }
                    "RDATE" => {
                        for date in p
                            .value
                            .as_deref()
                            .context("empty timezone RDATE")?
                            .split(',')
                        {
                            set = set.rdate(Tz::UTC.from_utc_datetime(&naive(date)?));
                        }
                    }
                    _ => {}
                }
            }
            let expanded = set.before(through).all(8192);
            if expanded.limited {
                bail!("timezone transition expansion exceeded safety limit");
            }
            // Include DTSTART even when it is after the query, to know the prior offset.
            let mut dates = expanded.dates;
            dates.push(start);
            for date in dates {
                transitions.push(Transition {
                    instant: date.with_timezone(&Utc) - Duration::seconds(from as i64),
                    from,
                    to,
                });
            }
            offsets.extend([from, to]);
        }
        transitions.sort_by_key(|transition| transition.instant);
        transitions.dedup_by_key(|transition| transition.instant);
        offsets.sort();
        offsets.dedup();
        if transitions.is_empty() {
            bail!("timezone contains no observances");
        }
        zones.insert(
            name,
            Zone::Custom(Arc::new(CustomZone {
                transitions,
                offsets,
            })),
        );
    }
    Ok(zones)
}
