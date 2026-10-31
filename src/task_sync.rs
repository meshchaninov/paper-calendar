use crate::{atomic_write, Calendar, Config, Event, Screen};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub uid: String,
    pub collection: String,
    pub resource: String,
    pub completed: bool,
}
pub type Outbox = BTreeMap<String, Change>;
pub fn load(cache: &Path) -> Result<Outbox> {
    match std::fs::read(cache.join("task-outbox.json")) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Outbox::new()),
        Err(e) => Err(e.into()),
    }
}
pub fn save(cache: &Path, outbox: &Outbox) -> Result<()> {
    atomic_write(
        &cache.join("task-outbox.json"),
        &serde_json::to_vec(outbox)?,
    )
}
pub fn apply_mark(screen: &mut Screen, id: &str, completed: bool) {
    if completed {
        screen.completed.insert(id.into());
    } else {
        screen.completed.remove(id);
        screen.completed_until.remove(id);
    }
}
pub fn reconcile(screen: &mut Screen, events: &[Event], cache: &Path) -> Result<()> {
    if screen.migrate_marks {
        for event in events.iter().filter(|e| e.task && !e.uid.is_empty()) {
            let id = event.task_id();
            if screen.completed.contains(&id)
                && !event.server_completed
                && !screen.outbox.contains_key(&id)
            {
                screen.outbox.insert(
                    id,
                    Change {
                        uid: event.uid.clone(),
                        collection: event.collection_url.clone(),
                        resource: event.resource_url.clone(),
                        completed: true,
                    },
                );
            }
        }
        save(cache, &screen.outbox)?;
        screen.migrate_marks = false;
        if !screen.outbox.is_empty() {
            screen.next_task_sync = chrono::Utc::now().timestamp() + 3;
        }
    }
    let mut updated_resource = false;
    for event in events.iter().filter(|e| e.task) {
        if let Some(change) = screen.outbox.get_mut(&event.task_id()) {
            if change.collection == event.collection_url
                && !event.resource_url.is_empty()
                && change.resource != event.resource_url
            {
                change.resource = event.resource_url.clone();
                updated_resource = true;
            }
        }
    }
    if updated_resource {
        save(cache, &screen.outbox)?;
        screen.next_task_sync = chrono::Utc::now().timestamp() + 3;
    }
    for event in events.iter().filter(|e| e.task) {
        let id = event.task_id();
        let completed = screen
            .outbox
            .get(&id)
            .map_or(event.server_completed, |c| c.completed);
        apply_mark(screen, &id, completed);
    }
    atomic_write(
        &cache.join("completed-tasks.json"),
        &serde_json::to_vec(&screen.completed)?,
    )?;
    Ok(())
}
pub fn resource_url(source: &Calendar, href: &str) -> Result<reqwest::Url> {
    let base = reqwest::Url::parse(&source.url)?;
    let url = base.join(href)?;
    ensure!(
        base.origin() == url.origin()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url
                .path()
                .starts_with(&format!("{}/", base.path().trim_end_matches('/')))
            && url.path() != base.path(),
        "Invalid task resource location"
    );
    Ok(url)
}

// Preserve the full original VCALENDAR, folded lines, alarms, vendor fields and
// unrelated properties. Replace completion metadata only in the matching VTODO.
pub fn edit_completion(ics: &str, uid: &str, completed: bool, now: &str) -> Result<String> {
    ensure!(!uid.is_empty(), "Task UID missing");
    let mut groups: Vec<(String, String)> = Vec::new();
    for line in ics.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with([' ', '\t']) {
            let previous = groups.last_mut().context("Invalid folded line")?;
            previous.0.push_str(line);
            previous.0.push_str("\r\n");
            previous.1.push_str(&line[1..]);
        } else {
            groups.push((format!("{line}\r\n"), line.into()));
        }
    }
    let mut ranges = Vec::new();
    let mut i = 0;
    while i < groups.len() {
        if groups[i].1.eq_ignore_ascii_case("BEGIN:VTODO") {
            let start = i;
            let mut depth = 1;
            let mut found_uid = false;
            let mut recurring = false;
            i += 1;
            while i < groups.len() && depth > 0 {
                let line = &groups[i].1;
                if depth == 1 {
                    if let Some((key, value)) = line.split_once(':') {
                        let name = key.split(';').next().unwrap_or("");
                        if name.eq_ignore_ascii_case("UID") && value == uid {
                            found_uid = true;
                        }
                        if ["RRULE", "RDATE", "EXDATE", "RECURRENCE-ID"]
                            .iter()
                            .any(|p| name.eq_ignore_ascii_case(p))
                        {
                            recurring = true;
                        }
                    }
                }
                if line.to_ascii_uppercase().starts_with("BEGIN:") {
                    depth += 1;
                }
                if line.to_ascii_uppercase().starts_with("END:") {
                    depth -= 1;
                }
                i += 1;
            }
            ensure!(depth == 0, "Incomplete VTODO");
            if found_uid {
                ensure!(!recurring, "Recurring task requires instance-aware updates");
                ranges.push((start, i));
            }
        } else {
            i += 1;
        }
    }
    ensure!(ranges.len() == 1, "Expected exactly one matching VTODO");
    let (start, end) = ranges[0];
    let mut out = String::new();
    let mut depth = 0;
    for (i, (raw, line)) in groups.iter().enumerate() {
        if i == end - 1 {
            out.push_str(if completed {
                "STATUS:COMPLETED\r\nPERCENT-COMPLETE:100\r\n"
            } else {
                "STATUS:NEEDS-ACTION\r\nPERCENT-COMPLETE:0\r\n"
            });
            if completed {
                out.push_str(&format!("COMPLETED:{now}\r\n"));
            }
            out.push_str(&format!("LAST-MODIFIED:{now}\r\nDTSTAMP:{now}\r\n"));
        }
        let in_target = i > start && i < end - 1;
        let key = line
            .split(':')
            .next()
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("");
        let replace = in_target
            && depth == 1
            && [
                "STATUS",
                "COMPLETED",
                "PERCENT-COMPLETE",
                "LAST-MODIFIED",
                "DTSTAMP",
            ]
            .iter()
            .any(|p| key.eq_ignore_ascii_case(p));
        if !replace {
            out.push_str(raw);
        }
        if i == start {
            depth = 1;
        } else if in_target {
            if line.to_ascii_uppercase().starts_with("BEGIN:") {
                depth += 1;
            }
            if line.to_ascii_uppercase().starts_with("END:") {
                depth -= 1;
            }
        }
    }
    Ok(out)
}

pub async fn send(client: &reqwest::Client, config: &Config, change: &Change) -> Result<()> {
    let source = config
        .calendars
        .iter()
        .find(|c| c.url == change.collection)
        .context("Task calendar removed")?;
    let user = std::env::var(&source.username_env).context("Username missing")?;
    let password = std::env::var(&source.password_env).context("Password missing")?;
    let resource = if change.resource.is_empty() {
        let uid = change
            .uid
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;");
        let body = format!(
            r#"<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/></d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VTODO"><c:prop-filter name="UID"><c:text-match collation="i;octet">{uid}</c:text-match></c:prop-filter></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>"#
        );
        let response = client
            .request(reqwest::Method::from_bytes(b"REPORT")?, &source.url)
            .basic_auth(&user, Some(&password))
            .header("Depth", "1")
            .header("Content-Type", "application/xml")
            .body(body)
            .send()
            .await?
            .error_for_status()?;
        ensure!(
            response.status() == reqwest::StatusCode::MULTI_STATUS,
            "Expected UID lookup response"
        );
        let bytes = response.bytes().await?;
        ensure!(bytes.len() <= 1024 * 1024, "UID lookup too large");
        let doc = roxmltree::Document::parse(std::str::from_utf8(&bytes)?)?;
        let hrefs: Vec<_> = doc
            .descendants()
            .filter(|n| n.has_tag_name(("DAV:", "response")))
            .filter_map(|n| {
                n.children()
                    .find(|c| c.has_tag_name(("DAV:", "href")))
                    .and_then(|n| n.text())
            })
            .collect();
        ensure!(hrefs.len() == 1, "Expected unique task UID");
        hrefs[0].to_owned()
    } else {
        change.resource.clone()
    };
    let url = resource_url(source, &resource)?;
    // A 412 means another client changed the task: re-read it and merge only the
    // completion fields, retaining that client's title, notes, dates and alarms.
    for _ in 0..3 {
        let response = client
            .get(url.clone())
            .basic_auth(&user, Some(&password))
            .send()
            .await?
            .error_for_status()?;
        ensure!(
            response.status() == reqwest::StatusCode::OK,
            "Expected task content"
        );
        let etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .context("Task ETag missing")?
            .to_str()?
            .to_owned();
        ensure!(
            etag.starts_with('"') && etag.ends_with('"') && !etag.starts_with("W/"),
            "Strong task ETag required"
        );
        ensure!(
            response
                .content_length()
                .is_none_or(|n| n <= 8 * 1024 * 1024),
            "Task too large"
        );
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= 8 * 1024 * 1024,
                "Task too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        let body = edit_completion(
            std::str::from_utf8(&bytes)?,
            &change.uid,
            change.completed,
            &chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string(),
        )?;
        let response = client
            .put(url.clone())
            .basic_auth(&user, Some(&password))
            .header(reqwest::header::IF_MATCH, &etag)
            .header(
                reqwest::header::CONTENT_TYPE,
                "text/calendar; charset=utf-8",
            )
            .body(body)
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::PRECONDITION_FAILED {
            continue;
        }
        ensure!(
            matches!(
                response.status(),
                reqwest::StatusCode::OK
                    | reqwest::StatusCode::CREATED
                    | reqwest::StatusCode::NO_CONTENT
            ),
            "Task update was not confirmed"
        );
        return Ok(());
    }
    anyhow::bail!("Task changed concurrently; retry later")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migrate_old_marks_once_then_accept_remote_reopen_and_pending_override() {
        let cache =
            std::env::temp_dir().join(format!("paper-calendar-migrate-{}", std::process::id()));
        std::fs::create_dir_all(&cache).unwrap();
        let config: Config = crate::test_config();
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let mut event = crate::demo_events(today, config.timezone)
            .into_iter()
            .find(|e| e.task)
            .unwrap();
        event.collection_url = config.calendars[0].url.clone();
        event.resource_url = format!("{}task.ics", event.collection_url);
        let id = event.task_id();
        let mut screen = Screen {
            migrate_marks: true,
            ..Screen::default()
        };
        screen.completed.insert(id.clone());
        reconcile(&mut screen, &[event.clone()], &cache).unwrap();
        assert!(screen.outbox[&id].completed);
        assert!(load(&cache).unwrap()[&id].completed);
        assert!(!screen.migrate_marks);
        screen.outbox.clear();
        save(&cache, &screen.outbox).unwrap();
        reconcile(&mut screen, &[event.clone()], &cache).unwrap();
        assert!(!screen.completed.contains(&id));
        event.server_completed = true;
        screen.outbox.insert(
            id.clone(),
            Change {
                uid: event.uid.clone(),
                collection: event.collection_url.clone(),
                resource: event.resource_url.clone(),
                completed: false,
            },
        );
        reconcile(&mut screen, &[event], &cache).unwrap();
        assert!(!screen.completed.contains(&id));
        let _ = std::fs::remove_dir_all(cache);
    }
    #[test]
    fn complete_and_undo_preserve_fields_and_alarm() {
        let original="BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTODO\r\nUID:task\r\nSUMMARY:Квартира\r\nDESCRIPTION:Long\r\n  note\r\nDUE;VALUE=DATE:20261016\r\nX-APPLE-TEST:keep\r\nSTATUS:IN-PROCESS\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:alarm\r\nEND:VALARM\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
        let done = edit_completion(original, "task", true, "20261031T030000Z").unwrap();
        for keep in [
            "SUMMARY:Квартира",
            "DESCRIPTION:Long\r\n  note",
            "DUE;VALUE=DATE:20261016",
            "X-APPLE-TEST:keep",
            "BEGIN:VALARM\r\nACTION:DISPLAY",
        ] {
            assert!(done.contains(keep));
        }
        assert!(
            done.contains("STATUS:COMPLETED\r\nPERCENT-COMPLETE:100\r\nCOMPLETED:20261031T030000Z")
        );
        let undone = edit_completion(&done, "task", false, "20261031T040000Z").unwrap();
        assert!(undone.contains("STATUS:NEEDS-ACTION\r\nPERCENT-COMPLETE:0"));
        assert!(!undone.contains("\r\nCOMPLETED:"));
        assert_eq!(undone.matches("DTSTAMP:").count(), 1);
        assert!(edit_completion(original, "other", true, "now").is_err());
        assert!(edit_completion(
            &original.replace("UID:task", "UID:task\r\nRRULE:FREQ=DAILY"),
            "task",
            true,
            "now"
        )
        .is_err());
    }
    #[test]
    fn resource_must_remain_inside_calendar_and_origin() {
        let source:Calendar=serde_json::from_str(r#"{"name":"test","url":"https://example.com/user/cal/","username_env":"U","password_env":"P"}"#).unwrap();
        assert!(resource_url(&source, "/user/cal/task.ics").is_ok());
        for path in [
            "https://evil.example/user/cal/task.ics",
            "../other/task.ics",
            "/user/cal/",
            "/user/cal/task.ics?other",
        ] {
            assert!(resource_url(&source, path).is_err());
        }
    }
}
