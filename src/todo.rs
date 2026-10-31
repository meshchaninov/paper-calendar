use crate::{atomic_write, render, Json, Screen, Shared};
use anyhow::Result;
use axum::{extract::State, http::StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::Path};

pub fn load(cache: &Path) -> Result<BTreeSet<String>> {
    match std::fs::read(cache.join("completed-tasks.json")) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeSet::new()),
        Err(e) => Err(e.into()),
    }
}
fn visible_events(screen: &Screen) -> Vec<crate::Event> {
    screen
        .view
        .as_ref()
        .map(|v| {
            v.events
                .iter()
                .filter(|e| {
                    !e.task
                        || !screen.completed.contains(&e.task_id())
                        || screen.completed_until.contains_key(&e.task_id())
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}
// Run only at a completion deadline; no periodic screen refresh is needed.
pub fn expire_completed(screen: &mut Screen, now: i64) -> Result<()> {
    let old = screen.completed_until.clone();
    screen.completed_until.retain(|_, deadline| *deadline > now);
    if old.len() == screen.completed_until.len() {
        return Ok(());
    }
    let previous_page = screen.selection.task_page;
    if let Some(view) = screen.view.as_ref() {
        let events = visible_events(screen);
        let layout = render::sidebar_layout(
            &events,
            screen.selection.day.unwrap_or(view.today),
            view.config.timezone,
        );
        let pages = render::tasks(&events)
            .len()
            .div_ceil(layout.task_limit)
            .max(1);
        screen.selection.task_page = screen.selection.task_page.min(pages - 1);
    }
    if let Err(error) = redraw(screen) {
        screen.completed_until = old;
        screen.selection.task_page = previous_page;
        return Err(error);
    }
    Ok(())
}
pub fn activity(screen: &mut Screen) {
    let deadline = chrono::Utc::now().timestamp() + 30;
    screen.idle_until = Some(deadline);
    screen.next_check = if screen.next_check == 0 {
        deadline
    } else {
        screen.next_check.min(deadline)
    };
    screen.wake.notify_one();
}
pub fn return_home_if_idle(screen: &mut Screen, now: i64) -> Result<()> {
    if !screen.idle_until.is_some_and(|deadline| now >= deadline) {
        return Ok(());
    }
    screen.idle_until = None;
    if screen.selection == render::Selection::default() {
        return Ok(());
    }
    let previous = screen.selection.clone();
    screen.selection = render::Selection::default();
    if let Err(error) = redraw(screen) {
        screen.selection = previous;
        screen.idle_until = Some(now + 1);
        return Err(error);
    }
    screen.wake.notify_one();
    Ok(())
}
pub fn redraw(screen: &mut Screen) -> Result<()> {
    if screen.selection.detail.as_ref().is_some_and(|id| {
        !screen
            .view
            .as_ref()
            .is_some_and(|v| v.events.iter().any(|e| e.detail_id() == *id))
    }) {
        screen.selection.detail = None;
        screen.selection.detail_page = 0;
    }
    let Some(view) = screen.view.as_ref() else {
        return Ok(());
    };
    let hidden_tasks = screen
        .completed
        .iter()
        .filter(|id| !screen.completed_until.contains_key(*id))
        .cloned()
        .collect();
    let svg = render::svg(
        &view.events,
        view.today,
        view.config.timezone,
        &view.config.font_family,
        screen.error,
        screen.demo,
        &render::RenderState {
            completed: &screen.completed,
            hidden_tasks: &hidden_tasks,
            pending_tasks: screen.outbox.len(),
            selection: &screen.selection,
        },
    );
    let etag = format!("\"{:x}\"", Sha256::digest(svg.as_bytes()));
    if etag == screen.etag {
        screen.metrics.unchanged_checks += 1;
        return Ok(());
    }
    let render_started = std::time::Instant::now();
    let (png, rgba) = if screen.native && !view.config.web_enabled {
        let rgba = render::raster(&svg, &view.config.font_path)?;
        (Vec::new(), rgba)
    } else {
        render::png_and_pixels(&svg, &view.config.font_path)?
    };
    if screen.rgba == rgba && !screen.rgba.is_empty() {
        screen.etag = etag;
        screen.metrics.unchanged_checks += 1;
        return Ok(());
    }
    if !screen.native || view.config.web_enabled {
        atomic_write(&view.config.cache_dir.join("calendar.png"), &png)?;
        atomic_write(&view.config.cache_dir.join("calendar.svg"), svg.as_bytes())?;
    }
    screen.metrics.last_render_ms = render_started.elapsed().as_millis() as u64;
    screen.metrics.image_renders += 1;
    screen.rgba = rgba;
    screen.png = png;
    screen.etag = etag;
    Ok(())
}
#[derive(Serialize)]
pub struct Task {
    id: String,
    title: String,
    completed: bool,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}
pub fn tasks(screen: &Screen) -> Vec<Task> {
    let Some(view) = screen.view.as_ref() else {
        return Vec::new();
    };
    if screen.selection.detail.is_some() {
        return Vec::new();
    }
    let layout = render::sidebar_layout(
        &view.events,
        screen.selection.day.unwrap_or(view.today),
        view.config.timezone,
    );
    let events = visible_events(screen);
    render::task_page(&events, screen.selection.task_page, layout.task_limit)
        .into_iter()
        .enumerate()
        .map(|(i, e)| {
            let id = e.task_id();
            let completed = screen.completed.contains(&id);
            Task {
                id,
                title: e.title.clone(),
                completed,
                x: render::TASK_X,
                y: layout.task_top + i as i32 * render::TASK_HEIGHT,
                width: 356,
                height: render::TASK_HEIGHT,
            }
        })
        .collect()
}
pub fn set_completed(screen: &mut Screen, id: &str, completed: bool) -> Result<bool> {
    activity(screen);
    if !screen
        .view
        .as_ref()
        .is_some_and(|v| v.events.iter().any(|e| e.task && e.task_id() == id))
    {
        return Ok(false);
    }
    if screen.completed.contains(id) == completed {
        return Ok(true);
    }
    let old = screen.completed.clone();
    let old_until = screen.completed_until.clone();
    let old_outbox = screen.outbox.clone();
    if completed {
        screen.completed.insert(id.into());
        let deadline = chrono::Utc::now().timestamp() + 30;
        screen.completed_until.insert(id.into(), deadline);
        screen.next_check = if screen.next_check == 0 {
            deadline
        } else {
            screen.next_check.min(deadline)
        };
    } else {
        screen.completed.remove(id);
        screen.completed_until.remove(id);
    }
    if !screen.demo {
        let view = screen.view.as_ref().unwrap();
        let event = view
            .events
            .iter()
            .find(|e| e.task && e.task_id() == id)
            .unwrap();
        if !event.uid.is_empty() {
            if let Some(source) = view.config.calendars.iter().find(|c| {
                if event.collection_url.is_empty() {
                    c.name == event.calendar
                } else {
                    c.url == event.collection_url
                }
            }) {
                screen.outbox.insert(
                    id.into(),
                    crate::task_sync::Change {
                        uid: event.uid.clone(),
                        collection: source.url.clone(),
                        resource: event.resource_url.clone(),
                        completed,
                    },
                );
                screen.next_task_sync = chrono::Utc::now().timestamp() + 3;
            }
        }
    }
    let result = (|| -> Result<()> {
        let view = screen.view.as_ref().unwrap();
        if screen.outbox != old_outbox {
            crate::task_sync::save(&view.config.cache_dir, &screen.outbox)?;
        }
        atomic_write(
            &view.config.cache_dir.join("completed-tasks.json"),
            &serde_json::to_vec(&screen.completed)?,
        )?;
        redraw(screen)
    })();
    if let Err(e) = result {
        screen.completed = old;
        screen.completed_until = old_until;
        screen.outbox = old_outbox;
        if let Some(view) = &screen.view {
            let _ = crate::task_sync::save(&view.config.cache_dir, &screen.outbox);
        }
        if let Some(view) = &screen.view {
            let _ = atomic_write(
                &view.config.cache_dir.join("completed-tasks.json"),
                &serde_json::to_vec(&screen.completed)?,
            );
        }
        return Err(e);
    }
    screen.wake.notify_one();
    Ok(true)
}
pub async fn list(State(shared): State<Shared>) -> Json<Vec<Task>> {
    Json(tasks(&*shared.read().await))
}
#[derive(Deserialize)]
pub struct Completion {
    id: String,
    completed: bool,
}
pub async fn complete(State(shared): State<Shared>, Json(body): Json<Completion>) -> StatusCode {
    match set_completed(&mut *shared.write().await, &body.id, body.completed) {
        Ok(true) => StatusCode::NO_CONTENT,
        Ok(false) => StatusCode::NOT_FOUND,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
// Keep the real current date separate from the month being browsed.
#[cfg(any(feature = "pocketbook", test))]
pub fn change_month(screen: &mut Screen, delta: i32) -> Result<()> {
    activity(screen);
    use chrono::{Datelike, Months};
    let Some(view) = screen.view.as_ref() else {
        return Ok(());
    };
    let month = screen
        .selection
        .month
        .unwrap_or(view.today)
        .with_day(1)
        .unwrap();
    let target = if delta < 0 {
        month.checked_sub_months(Months::new(delta.unsigned_abs()))
    } else {
        month.checked_add_months(Months::new(delta as u32))
    };
    let Some(target) = target else {
        return Ok(());
    };
    screen.selection.month = Some(target);
    screen.selection.day = Some(target);
    screen.selection.agenda_page = 0;
    screen.selection.task_page = 0;
    screen.selection.detail = None;
    screen.selection.detail_page = 0;
    redraw(screen)?;
    screen.wake.notify_one();
    Ok(())
}
pub fn tap(screen: &mut Screen, x: i32, y: i32) -> Result<()> {
    activity(screen);
    if let Some(task) = tasks(screen)
        .into_iter()
        .find(|t| x >= t.x && x < t.x + t.width && y >= t.y && y < t.y + t.height)
    {
        set_completed(screen, &task.id, !task.completed)?;
        return Ok(());
    }
    let Some(view) = screen.view.as_ref() else {
        return Ok(());
    };
    let Some(control) = context_controls(screen)
        .into_iter()
        .find(|c| c.contains(x, y))
    else {
        return Ok(());
    };
    let day = screen.selection.day.unwrap_or(view.today);
    let agenda_pages = render::agenda_pages(&view.events, day, view.config.timezone).len();
    let layout = render::sidebar_layout(&view.events, day, view.config.timezone);
    let visible = visible_events(screen);
    let task_pages = render::tasks(&visible)
        .len()
        .div_ceil(layout.task_limit)
        .max(1);
    match control.action {
        render::Action::Day(day) => {
            screen.selection.day = Some(day);
            screen.selection.agenda_page = 0;
            screen.selection.detail = None;
        }
        render::Action::Refresh => {
            if !screen.busy {
                screen.manual_refresh = true;
                screen.next_check = chrono::Utc::now().timestamp();
                screen.wake.notify_one();
            }
            return Ok(());
        }
        render::Action::Today => {
            screen.selection.month = None;
            screen.wake.notify_one();
            screen.selection.day = None;
            screen.selection.agenda_page = 0;
            screen.selection.detail = None;
        }
        render::Action::AgendaPrevious => {
            screen.selection.agenda_page =
                (screen.selection.agenda_page % agenda_pages + agenda_pages - 1) % agenda_pages
        }
        render::Action::AgendaNext => {
            screen.selection.agenda_page = (screen.selection.agenda_page + 1) % agenda_pages
        }
        render::Action::TasksPrevious => {
            screen.selection.task_page =
                (screen.selection.task_page % task_pages + task_pages - 1) % task_pages
        }
        render::Action::TasksNext => {
            screen.selection.task_page = (screen.selection.task_page + 1) % task_pages
        }
        render::Action::Open(id) => {
            screen.selection.detail = Some(id);
            screen.selection.detail_page = 0;
        }
        render::Action::Back => screen.selection.detail = None,
        render::Action::DetailPrevious | render::Action::DetailNext => {
            let count = screen
                .selection
                .detail
                .as_ref()
                .and_then(|id| view.events.iter().find(|e| e.detail_id() == *id))
                .map(|e| {
                    render::detail_lines(e, view.config.timezone)
                        .len()
                        .div_ceil(render::DETAIL_LINES)
                })
                .unwrap_or(1)
                .max(1);
            let direction = if matches!(control.action, render::Action::DetailPrevious) {
                count - 1
            } else {
                1
            };
            screen.selection.detail_page = (screen.selection.detail_page + direction) % count;
        }
        render::Action::Toggle(id) => {
            let done = screen.completed.contains(&id);
            set_completed(screen, &id, !done)?;
        }
    }
    redraw(screen)?;
    Ok(())
}
pub fn context_controls(screen: &Screen) -> Vec<render::Control> {
    let Some(view) = screen.view.as_ref() else {
        return Vec::new();
    };
    let month = screen.selection.month.unwrap_or(view.today);
    let mut out = render::calendar_task_controls(&view.events, month, view.config.timezone);
    out.extend(render::controls(month));
    let day = screen.selection.day.unwrap_or(view.today);
    let layout = render::sidebar_layout(&view.events, day, view.config.timezone);
    if screen.selection.detail.is_none() {
        if render::agenda_pages(&view.events, day, view.config.timezone).len() > 1 {
            out.extend(render::paging_controls(
                layout.agenda_nav,
                render::Action::AgendaPrevious,
                render::Action::AgendaNext,
                "события",
            ));
        }
        if render::tasks(&visible_events(screen)).len() > layout.task_limit {
            out.extend(render::paging_controls(
                layout.task_header,
                render::Action::TasksPrevious,
                render::Action::TasksNext,
                "задачи",
            ));
        }
    }
    let mut add = |label: String, x, y, width, height, action| {
        out.push(render::Control {
            label,
            x,
            y,
            width,
            height,
            action,
        })
    };
    if let Some(event) = screen
        .selection
        .detail
        .as_ref()
        .and_then(|id| view.events.iter().find(|e| e.detail_id() == *id))
    {
        // Keep month selection available but remove list pagination underneath details.
        out.retain(|c| {
            matches!(
                c.action,
                render::Action::Day(_)
                    | render::Action::Today
                    | render::Action::Refresh
                    | render::Action::Toggle(_)
            )
        });
        let mut add = |label: String, x, y, width, height, action| {
            out.push(render::Control {
                label,
                x,
                y,
                width,
                height,
                action,
            })
        };
        add(
            "Назад к списку".into(),
            1490,
            25,
            338,
            65,
            render::Action::Back,
        );
        add(
            "Предыдущая страница описания".into(),
            1490,
            1280,
            96,
            68,
            render::Action::DetailPrevious,
        );
        add(
            "Следующая страница описания".into(),
            1732,
            1280,
            96,
            68,
            render::Action::DetailNext,
        );
        if event.task {
            add(
                "Изменить выполнение задачи".into(),
                1490,
                1195,
                338,
                70,
                render::Action::Toggle(event.task_id()),
            );
        }
    } else {
        let day = screen.selection.day.unwrap_or(view.today);
        let pages = render::agenda_pages(&view.events, day, view.config.timezone);
        for row in pages[screen.selection.agenda_page % pages.len()]
            .iter()
            .filter(|r| !r.event.all_day && !r.event.task)
        {
            add(
                format!("Открыть: {}", row.event.title),
                1480,
                if row.event.all_day { row.y } else { row.y - 28 },
                356,
                if row.event.all_day { 68 } else { 50 },
                render::Action::Open(row.event.detail_id()),
            );
        }
    }
    out
}
#[cfg(feature = "pocketbook")]
pub fn back(screen: &mut Screen) -> Result<bool> {
    activity(screen);
    if screen.selection.detail.take().is_some() {
        redraw(screen)?;
        Ok(true)
    } else {
        Ok(false)
    }
}
pub async fn controls(State(shared): State<Shared>) -> Json<Vec<render::Control>> {
    Json(context_controls(&*shared.read().await))
}
#[derive(Deserialize)]
pub struct Tap {
    x: i32,
    y: i32,
}
pub async fn handle_tap(State(shared): State<Shared>, Json(body): Json<Tap>) -> StatusCode {
    match tap(&mut *shared.write().await, body.x, body.y) {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_without_web_renders_without_png_or_image_file_writes() {
        let mut config: crate::Config = crate::test_config();
        let cache =
            std::env::temp_dir().join(format!("paper-calendar-direct-{}", std::process::id()));
        std::fs::create_dir_all(&cache).unwrap();
        config.cache_dir = cache.clone();
        config.web_enabled = false;
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let events = crate::demo_events(today, config.timezone);
        let mut screen = Screen {
            native: true,
            view: Some(crate::View {
                events,
                today,
                config,
            }),
            ..Screen::default()
        };
        redraw(&mut screen).unwrap();
        assert!(screen.png.is_empty());
        assert_eq!(screen.rgba.len(), 1872 * 1404 * 4);
        assert!(!cache.join("calendar.png").exists());
        assert!(!cache.join("calendar.svg").exists());
        let renders = screen.metrics.image_renders;
        redraw(&mut screen).unwrap();
        assert_eq!(screen.metrics.image_renders, renders);
        let _ = std::fs::remove_dir_all(cache);
    }
    #[test]
    fn idle_returns_to_today_preserves_marks_and_new_touch_extends_deadline() {
        let mut config: crate::Config = crate::test_config();
        let cache =
            std::env::temp_dir().join(format!("paper-calendar-idle-{}", std::process::id()));
        std::fs::create_dir_all(&cache).unwrap();
        config.cache_dir = cache.clone();
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let events = crate::demo_events(today, config.timezone);
        let mut screen = Screen {
            view: Some(crate::View {
                events,
                today,
                config,
            }),
            ..Screen::default()
        };
        screen.completed.insert("keep-this-mark".into());
        redraw(&mut screen).unwrap();
        let home = screen.etag.clone();
        change_month(&mut screen, 1).unwrap();
        screen.selection.agenda_page = 2;
        screen.selection.task_page = 1;
        screen.selection.detail = Some("open-description".into());
        screen.selection.detail_page = 2;
        let deadline = screen.idle_until.unwrap();
        let selection = screen.selection.clone();
        return_home_if_idle(&mut screen, deadline - 1).unwrap();
        assert!(screen.selection == selection);
        screen.idle_until = Some(deadline - 20);
        tap(&mut screen, -1, -1).unwrap(); // Even an empty touch counts as activity.
        assert!(screen.idle_until.unwrap() > deadline - 20);
        let deadline = screen.idle_until.unwrap();
        return_home_if_idle(&mut screen, deadline).unwrap();
        assert!(screen.selection == render::Selection::default());
        assert!(screen.idle_until.is_none());
        assert_eq!(screen.etag, home);
        assert!(screen.completed.contains("keep-this-mark"));
        let renders = screen.metrics.image_renders;
        activity(&mut screen);
        let deadline = screen.idle_until.unwrap();
        return_home_if_idle(&mut screen, deadline).unwrap();
        assert_eq!(screen.metrics.image_renders, renders);
        let _ = std::fs::remove_dir_all(cache);
    }
    #[test]
    fn calendar_task_tap_matches_row_and_completion_is_shared_with_sidebar() {
        let mut config: crate::Config = crate::test_config();
        let cache =
            std::env::temp_dir().join(format!("paper-calendar-cell-{}", std::process::id()));
        std::fs::create_dir_all(&cache).unwrap();
        config.cache_dir = cache.clone();
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let day = chrono::NaiveDate::from_ymd_opt(2026, 10, 16).unwrap();
        let mut events: Vec<_> = crate::demo_events(today, config.timezone)
            .into_iter()
            .filter(|e| e.task)
            .take(3)
            .collect();
        events[0].title = "Заплатить за квартиру".into();
        events[0].task_date = Some(day);
        events[1].task_date = Some(day);
        events[2].task_date = None;
        let first = events[0].task_id();
        let second = events[1].task_id();
        let mut screen = Screen {
            view: Some(crate::View {
                events,
                today,
                config,
            }),
            ..Screen::default()
        };
        let controls = context_controls(&screen);
        let cells: Vec<_> = controls
            .iter()
            .filter(|c| matches!(c.action, render::Action::Toggle(_)))
            .collect();
        assert_eq!(cells.len(), 2);
        assert!(cells.iter().all(|c| c.x < 1455 && c.height >= 38));
        let target = cells
            .iter()
            .find(|c| matches!(&c.action,render::Action::Toggle(id) if *id==first))
            .unwrap();
        let (x, y) = (target.x + target.width - 10, target.y + target.height / 2);
        tap(&mut screen, x, y).unwrap();
        assert!(screen.completed.contains(&first));
        assert!(!screen.completed.contains(&second));
        assert!(screen.selection.day.is_none()); // Day selection must not steal the tap.
        assert!(tasks(&screen).iter().any(|t| t.id == first && t.completed));
        let deadline = screen.completed_until[&first];
        expire_completed(&mut screen, deadline).unwrap();
        assert!(!tasks(&screen).iter().any(|t| t.id == first));
        assert!(context_controls(&screen)
            .iter()
            .any(|c| matches!(&c.action,render::Action::Toggle(id) if *id==first)));
        tap(&mut screen, x, y).unwrap(); // Calendar retains checked task and can undo later.
        assert!(!screen.completed.contains(&first));
        assert!(tasks(&screen).iter().any(|t| t.id == first && !t.completed));
        let _ = std::fs::remove_dir_all(cache);
    }
    #[test]
    fn completion_expires_after_grace_and_can_be_cancelled() {
        let mut config: crate::Config = crate::test_config();
        let cache =
            std::env::temp_dir().join(format!("paper-calendar-expiry-{}", std::process::id()));
        std::fs::create_dir_all(&cache).unwrap();
        config.cache_dir = cache.clone();
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        let events = crate::demo_events(today, config.timezone);
        let mut screen = Screen {
            view: Some(crate::View {
                events,
                today,
                config,
            }),
            ..Screen::default()
        };
        let first = tasks(&screen)[0].id.clone();
        set_completed(&mut screen, &first, true).unwrap();
        let deadline = screen.completed_until[&first];
        let marked = screen.etag.clone();
        expire_completed(&mut screen, deadline - 1).unwrap();
        assert!(tasks(&screen).iter().any(|t| t.id == first && t.completed));
        assert_eq!(screen.etag, marked);
        set_completed(&mut screen, &first, false).unwrap();
        expire_completed(&mut screen, deadline).unwrap();
        assert!(tasks(&screen).iter().any(|t| t.id == first && !t.completed));
        set_completed(&mut screen, &first, true).unwrap();
        let deadline = screen.completed_until[&first];
        let before = screen.rgba.clone();
        expire_completed(&mut screen, deadline).unwrap();
        assert!(!tasks(&screen).iter().any(|t| t.id == first));
        assert!(screen.completed.contains(&first));
        let regions = crate::energy::changed_regions(&before, &screen.rgba, 1872, 1404);
        assert!(!regions.is_empty());
        assert!(regions.iter().all(|r| r.x >= 1472));
        assert_eq!(tasks(&screen).len(), 3); // Busy day keeps three task rows filled.
        screen.selection.task_page = 100;
        let remaining: Vec<_> = render::tasks(&visible_events(&screen))
            .iter()
            .map(|e| e.task_id())
            .collect();
        for id in &remaining {
            set_completed(&mut screen, id, true).unwrap();
        }
        expire_completed(&mut screen, chrono::Utc::now().timestamp() + 31).unwrap();
        assert!(tasks(&screen).is_empty());
        assert_eq!(screen.selection.task_page, 0);
        assert!(!context_controls(&screen).iter().any(|c| matches!(
            c.action,
            render::Action::TasksNext | render::Action::TasksPrevious
        )));
        let _ = std::fs::remove_dir_all(cache);
    }
    #[test]
    fn month_navigation_crosses_year_and_today_restores_current_date() {
        use chrono::NaiveDate;
        let mut config: crate::Config = crate::test_config();
        let cache =
            std::env::temp_dir().join(format!("paper-calendar-month-{}", std::process::id()));
        std::fs::create_dir_all(&cache).unwrap();
        config.cache_dir = cache.clone();
        let today = NaiveDate::from_ymd_opt(2026, 12, 31).unwrap();
        let mut screen = Screen {
            view: Some(crate::View {
                events: Vec::new(),
                today,
                config,
            }),
            ..Screen::default()
        };
        screen.completed.insert("saved-mark".into());
        change_month(&mut screen, 1).unwrap();
        assert_eq!(screen.selection.month, NaiveDate::from_ymd_opt(2027, 1, 1));
        assert_eq!(screen.view.as_ref().unwrap().today, today);
        let svg = std::fs::read_to_string(cache.join("calendar.svg")).unwrap();
        assert!(svg.contains("Январь 2027"));
        assert!(svg.contains("Сегодня · 31.12"));
        assert!(context_controls(&screen)
            .iter()
            .any(|c| c.label == "31.01.2027"));
        change_month(&mut screen, -1).unwrap();
        assert_eq!(screen.selection.month, NaiveDate::from_ymd_opt(2026, 12, 1));
        change_month(&mut screen, -10).unwrap();
        assert_eq!(screen.selection.month, NaiveDate::from_ymd_opt(2026, 2, 1));
        let today_control = context_controls(&screen)
            .into_iter()
            .find(|c| c.label == "Сегодня")
            .unwrap();
        tap(&mut screen, today_control.x + 10, today_control.y + 10).unwrap();
        assert!(screen.selection.month.is_none());
        assert!(screen.selection.day.is_none());
        assert!(screen.completed.contains("saved-mark"));
        let _ = std::fs::remove_dir_all(cache);
    }
    #[test]
    fn details_are_only_for_timed_events_and_tasks_fill_five_rows() {
        let config: crate::Config = crate::test_config();
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let events = crate::demo_events(today, config.timezone);
        let screen = Screen {
            view: Some(crate::View {
                events: events.clone(),
                today,
                config,
            }),
            selection: render::Selection {
                day: Some(chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()),
                ..render::Selection::default()
            },
            ..Screen::default()
        };
        let controls = context_controls(&screen);
        for c in &controls {
            if matches!(
                c.action,
                render::Action::TasksPrevious | render::Action::TasksNext
            ) {
                assert!(c.width >= 74 && c.height >= 64);
            }
        }
        for control in &controls {
            if let render::Action::Open(ref id) = control.action {
                let e = events.iter().find(|e| e.detail_id() == *id).unwrap();
                assert!(!e.task && !e.all_day);
            }
        }
        let first = tasks(&screen);
        assert_eq!(first.len(), 5);
        assert_eq!(first[0].y, 760);
        assert_eq!(first[4].y, 1192);
        assert!(first.iter().all(|t| t.width >= 300));
        let second = render::task_page(&events, 1, 5);
        assert_eq!(second.len(), 2);
        assert!(second
            .iter()
            .all(|e| !first.iter().any(|t| t.id == e.task_id())));
    }
}
