use crate::{month_range, Calendar, Config, Event};
use anyhow::{bail, ensure, Context, Result};
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};
use ical::property::Property;
use std::io::BufReader;

const CAL: &str = "urn:ietf:params:xml:ns:caldav";
fn value<'a>(properties: &'a [Property], name: &str) -> Option<&'a str> {
    properties
        .iter()
        .find(|p| p.name == name)
        .and_then(|p| p.value.as_deref())
}
fn date(prop: &Property, fallback: chrono_tz::Tz) -> Result<(DateTime<Utc>, bool)> {
    let s = prop.value.as_deref().context("Missing date value")?;
    if s.len() == 8 {
        let d = NaiveDate::parse_from_str(s, "%Y%m%d")?;
        return Ok((
            fallback
                .from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
                .single()
                .context("Ambiguous date")?
                .with_timezone(&Utc),
            true,
        ));
    }
    if s.ends_with('Z') {
        return Ok((
            Utc.from_utc_datetime(&NaiveDateTime::parse_from_str(s, "%Y%m%dT%H%M%SZ")?),
            false,
        ));
    }
    let tz = prop
        .params
        .as_ref()
        .and_then(|p| p.iter().find(|(key, _)| key == "TZID"))
        .and_then(|(_, values)| values.first())
        .map(|s| s.parse::<chrono_tz::Tz>())
        .transpose()?
        .unwrap_or(fallback);
    Ok((
        tz.from_local_datetime(&NaiveDateTime::parse_from_str(s, "%Y%m%dT%H%M%S")?)
            .single()
            .context("Ambiguous local event time")?
            .with_timezone(&Utc),
        false,
    ))
}
fn unescape(s: &str) -> String {
    let mut chars = s.chars();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n' | 'N') => out.push('\n'),
                Some(c) => out.push(c),
                None => out.push('\\'),
            }
        } else if !c.is_control() {
            out.push(c)
        }
    }
    out
}
fn duration(s: &str) -> Result<Duration> {
    ensure!(s.starts_with('P'), "Unsupported duration");
    let mut total = 0i64;
    let mut number = String::new();
    let mut time = false;
    for c in s[1..].chars() {
        if c == 'T' {
            time = true;
            continue;
        }
        if c.is_ascii_digit() {
            number.push(c);
            continue;
        }
        let n: i64 = number.parse()?;
        number.clear();
        let multiplier = match (c, time) {
            ('W', false) => 604800,
            ('D', false) => 86400,
            ('H', true) => 3600,
            ('M', true) => 60,
            ('S', true) => 1,
            _ => bail!("Unsupported duration"),
        };
        total = total
            .checked_add(n.checked_mul(multiplier).context("Duration overflow")?)
            .context("Duration overflow")?;
    }
    ensure!(number.is_empty(), "Invalid duration");
    Duration::try_seconds(total).context("Duration out of range")
}
fn component(
    props: &[Property],
    task: bool,
    source: &Calendar,
    config: &Config,
    today: NaiveDate,
) -> Result<Option<Event>> {
    if value(props, "STATUS") == Some("CANCELLED")
        || (!task && value(props, "STATUS") == Some("COMPLETED"))
    {
        return Ok(None);
    }
    ensure!(
        !props
            .iter()
            .any(|p| matches!(p.name.as_str(), "RRULE" | "RDATE" | "EXDATE")),
        "Server did not expand recurrences"
    );
    let start_prop = props
        .iter()
        .find(|p| p.name == if task { "DUE" } else { "DTSTART" })
        .or_else(|| {
            if task {
                props.iter().find(|p| p.name == "DTSTART")
            } else {
                None
            }
        });
    let (start, all_day) = match start_prop {
        Some(p) => date(p, config.timezone)?,
        None if task => (
            config
                .timezone
                .from_local_datetime(&today.and_hms_opt(12, 0, 0).unwrap())
                .single()
                .context("Invalid date")?
                .with_timezone(&Utc),
            true,
        ),
        None => bail!("Missing DTSTART"),
    };
    let end = if task {
        start + Duration::seconds(1)
    } else if let Some(p) = props.iter().find(|p| p.name == "DTEND") {
        date(p, config.timezone)?.0
    } else if let Some(s) = value(props, "DURATION") {
        start
            .checked_add_signed(duration(s)?)
            .context("End date out of range")?
    } else if all_day {
        let next = start
            .with_timezone(&config.timezone)
            .date_naive()
            .succ_opt()
            .context("Date out of range")?;
        config
            .timezone
            .from_local_datetime(&next.and_hms_opt(0, 0, 0).unwrap())
            .single()
            .context("Invalid all-day end")?
            .with_timezone(&Utc)
    } else {
        start
    };
    ensure!(end >= start, "DTEND precedes DTSTART");
    Ok(Some(Event {
        production_day: crate::production::classify(
            source.production_calendar,
            task,
            all_day,
            &unescape(value(props, "SUMMARY").unwrap_or("")),
        ),
        uid: value(props, "UID").unwrap_or("").to_string(),
        title: unescape(value(props, "SUMMARY").unwrap_or("Без названия")).replace('\n', " "),
        description: unescape(value(props, "DESCRIPTION").unwrap_or("")).replace('\r', ""),
        location: unescape(value(props, "LOCATION").unwrap_or("")).replace('\n', " "),
        calendar: source.name.clone(),
        start,
        end,
        all_day,
        task,
        collection_url: source.url.clone(),
        resource_url: String::new(),
        server_completed: task
            && (value(props, "STATUS") == Some("COMPLETED")
                || value(props, "COMPLETED").is_some()
                || value(props, "PERCENT-COMPLETE") == Some("100")),
        task_date: if task && start_prop.is_some() {
            Some(start.with_timezone(&config.timezone).date_naive())
        } else {
            None
        },
    }))
}
fn parse_ics(
    text: &str,
    source: &Calendar,
    config: &Config,
    today: NaiveDate,
    tasks: bool,
) -> Result<Vec<Event>> {
    let mut events = Vec::new();
    for calendar in ical::IcalParser::new(BufReader::new(text.as_bytes())) {
        let calendar = calendar?;
        if tasks {
            for task in calendar.todos {
                if let Some(e) = component(&task.properties, true, source, config, today)? {
                    events.push(e);
                }
            }
        } else {
            for event in calendar.events {
                if let Some(e) = component(&event.properties, false, source, config, today)? {
                    events.push(e);
                }
            }
        }
    }
    Ok(events)
}
fn parse_report(
    xml: &str,
    source: &Calendar,
    config: &Config,
    today: NaiveDate,
    tasks: bool,
) -> Result<Vec<Event>> {
    let doc = roxmltree::Document::parse(xml)?;
    ensure!(
        doc.root_element().has_tag_name(("DAV:", "multistatus")),
        "Expected DAV multistatus"
    );
    let mut out = Vec::new();
    for node in doc
        .descendants()
        .filter(|n| n.has_tag_name(("DAV:", "status")))
    {
        let status = node.text().unwrap_or("");
        ensure!(
            status
                .split_whitespace()
                .nth(1)
                .is_some_and(|s| s.starts_with('2')),
            "Failed CalDAV response member"
        );
    }
    for response in doc
        .descendants()
        .filter(|n| n.has_tag_name(("DAV:", "response")))
    {
        let href = response
            .children()
            .find(|n| n.has_tag_name(("DAV:", "href")))
            .and_then(|n| n.text());
        for node in response
            .descendants()
            .filter(|n| n.has_tag_name((CAL, "calendar-data")))
        {
            if let Some(text) = node.text() {
                let mut events = parse_ics(text, source, config, today, tasks)?;
                if let Some(href) = href {
                    let url = crate::task_sync::resource_url(source, href)?;
                    for e in &mut events {
                        e.resource_url = url.to_string();
                    }
                }
                out.extend(events);
            }
        }
    }
    Ok(out)
}
async fn report(
    client: &reqwest::Client,
    source: &Calendar,
    config: &Config,
    today: NaiveDate,
    tasks: bool,
) -> Result<Vec<Event>> {
    let (start, end) = month_range(today);
    let utc = |d: NaiveDate| -> Result<String> {
        Ok(config
            .timezone
            .from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
            .single()
            .context("Invalid range boundary")?
            .with_timezone(&Utc)
            .format("%Y%m%dT%H%M%SZ")
            .to_string())
    };
    let (start, end) = (utc(start)?, utc(end)?);
    let component = if tasks { "VTODO" } else { "VEVENT" };
    let expansion = if tasks {
        String::new()
    } else {
        format!(r#"<c:expand start="{start}" end="{end}"/>"#)
    };
    let range = if tasks {
        String::new()
    } else {
        format!(r#"<c:time-range start="{start}" end="{end}"/>"#)
    };
    let body = format!(
        r#"<?xml version="1.0"?><c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><c:calendar-data>{expansion}</c:calendar-data></d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="{component}">{range}</c:comp-filter></c:comp-filter></c:filter></c:calendar-query>"#
    );
    let user = std::env::var(&source.username_env).context("Username environment missing")?;
    let password = std::env::var(&source.password_env).context("Password environment missing")?;
    let url = reqwest::Url::parse(&source.url)?;
    ensure!(
        matches!(url.scheme(), "https" | "http")
            && url.username().is_empty()
            && url.password().is_none(),
        "Use HTTP(S) collection URL without embedded credentials"
    );
    let mut response = client
        .request(reqwest::Method::from_bytes(b"REPORT")?, url)
        .basic_auth(user, Some(password))
        .header("Depth", "1")
        .header("Content-Type", "application/xml; charset=utf-8")
        .body(body)
        .send()
        .await?
        .error_for_status()?;
    ensure!(
        response.status() == reqwest::StatusCode::MULTI_STATUS,
        "Expected 207 response"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= 8 * 1024 * 1024,
            "CalDAV response too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    parse_report(std::str::from_utf8(&bytes)?, source, config, today, tasks)
}
pub async fn fetch(
    client: &reqwest::Client,
    config: &Config,
    today: NaiveDate,
) -> Result<Vec<Event>> {
    let mut events = Vec::new();
    for source in &config.calendars {
        events.extend(report(client, source, config, today, false).await?);
        if source.tasks {
            events.extend(report(client, source, config, today, true).await?);
        }
    }
    events.sort_by(|a, b| (a.start, &a.calendar, &a.title).cmp(&(b.start, &b.calendar, &b.title)));
    events.dedup();
    Ok(events)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Config, NaiveDate) {
        (
            crate::test_config(),
            NaiveDate::from_ymd_opt(2026, 10, 31).unwrap(),
        )
    }
    #[test]
    fn task_date_requires_explicit_due_or_start_and_uses_local_timezone() {
        let (c, d) = fixture();
        let text="BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\nSUMMARY:Без даты\r\nEND:VTODO\r\nBEGIN:VTODO\r\nSUMMARY:Квартира\r\nDUE;VALUE=DATE:20261016\r\nEND:VTODO\r\nBEGIN:VTODO\r\nSUMMARY:Ночью\r\nDTSTART:20261015T190000Z\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
        let events = parse_ics(text, &c.calendars[0], &c, d, true).unwrap();
        assert_eq!(events[0].task_date, None);
        assert_eq!(events[1].task_date, NaiveDate::from_ymd_opt(2026, 10, 16));
        assert_eq!(events[2].task_date, NaiveDate::from_ymd_opt(2026, 10, 16));
    }
    #[test]
    fn description_preserves_paragraphs_and_location() {
        let (c, d) = fixture();
        let e=parse_ics("BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:details-test\r\nDTSTART:20261031T030000Z\r\nDESCRIPTION:Первая строка\\n\\nВторой абзац\r\nLOCATION:Кабинет 42\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",&c.calendars[0],&c,d,false).unwrap();
        assert_eq!(e[0].description, "Первая строка\n\nВторой абзац");
        assert_eq!(e[0].location, "Кабинет 42");
    }
    #[test]
    fn parses_utc_floating_all_day_folded_and_cancelled() {
        let (c, d) = fixture();
        let text = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nDTSTART:20261031T030000Z\r\nDTEND:20261031T040000Z\r\nSUMMARY:Встреча\\,\r\n  проект\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nDTSTART;VALUE=DATE:20261101\r\nSUMMARY:Выходной\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nDTSTART:20261031T100000\r\nSTATUS:CANCELLED\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let e = parse_ics(text, &c.calendars[0], &c, d, false).unwrap();
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].title, "Встреча, проект");
        assert_eq!(e[1].end - e[1].start, Duration::days(1));
        assert!(e[1].all_day);
    }
    #[test]
    fn refuses_unexpanded_recurrence() {
        let (c, d) = fixture();
        assert!(parse_ics("BEGIN:VCALENDAR\nBEGIN:VEVENT\nDTSTART:20261031T030000Z\nRRULE:FREQ=DAILY\nEND:VEVENT\nEND:VCALENDAR\n",&c.calendars[0],&c,d,false).is_err());
    }
    #[test]
    fn tasks_and_duration() {
        let (c, d) = fixture();
        let e = parse_ics("BEGIN:VCALENDAR\nBEGIN:VTODO\nSUMMARY:Задача\nEND:VTODO\nBEGIN:VTODO\nSTATUS:COMPLETED\nEND:VTODO\nEND:VCALENDAR\n",&c.calendars[0],&c,d,true).unwrap();
        assert_eq!(e.len(), 2);
        assert!(e[0].task);
        assert!(e[1].server_completed);
        assert_eq!(duration("P1DT2H30M").unwrap(), Duration::minutes(1590));
    }
    #[test]
    fn failed_multistatus_is_not_empty_success() {
        let (c, d) = fixture();
        assert!(parse_report("<d:multistatus xmlns:d=\"DAV:\"><d:response><d:status>HTTP/1.1 403 Forbidden</d:status></d:response></d:multistatus>",&c.calendars[0],&c,d,false).is_err());
    }
}
