use crate::{month_range, Event};
use anyhow::{Context, Result};
use chrono::{Datelike, Duration, NaiveDate, TimeZone, Utc};
use serde::Serialize;
use std::{collections::BTreeSet, fmt::Write};

pub const TASK_X: i32 = 1480;
pub const TASK_HEIGHT: i32 = 108;
const MONTHS: [&str; 12] = [
    "Январь",
    "Февраль",
    "Март",
    "Апрель",
    "Май",
    "Июнь",
    "Июль",
    "Август",
    "Сентябрь",
    "Октябрь",
    "Ноябрь",
    "Декабрь",
];
const MONTH_GENITIVE: [&str; 12] = [
    "января",
    "февраля",
    "марта",
    "апреля",
    "мая",
    "июня",
    "июля",
    "августа",
    "сентября",
    "октября",
    "ноября",
    "декабря",
];
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Selection {
    pub month: Option<NaiveDate>,
    pub day: Option<NaiveDate>,
    pub agenda_page: usize,
    pub task_page: usize,
    pub detail: Option<String>,
    pub detail_page: usize,
}
pub struct RenderState<'a> {
    pub completed: &'a BTreeSet<String>,
    pub hidden_tasks: &'a BTreeSet<String>,
    pub pending_tasks: usize,
    pub selection: &'a Selection,
}
#[derive(Clone, Serialize)]
pub enum Action {
    Day(NaiveDate),
    Today,
    Refresh,
    AgendaPrevious,
    AgendaNext,
    TasksPrevious,
    TasksNext,
    Open(String),
    Back,
    DetailPrevious,
    DetailNext,
    Toggle(String),
}
#[derive(Serialize)]
pub struct Control {
    pub label: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    #[serde(skip)]
    pub action: Action,
}
impl Control {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}
pub fn controls(today: NaiveDate) -> Vec<Control> {
    let start = month_range(today).0;
    let mut out: Vec<_> = (0..42)
        .map(|i| {
            let day = start + Duration::days(i as i64);
            Control {
                label: day.format("%d.%m.%Y").to_string(),
                x: 36 + (i % 7) * 199,
                y: 130 + (i / 7) * 198,
                width: 199,
                height: 198,
                action: Action::Day(day),
            }
        })
        .collect();
    out.push(Control {
        label: "Обновить".into(),
        x: 36,
        y: 20,
        width: 200,
        height: 60,
        action: Action::Refresh,
    });
    out.push(Control {
        label: "Сегодня".into(),
        x: 1090,
        y: 20,
        width: 339,
        height: 60,
        action: Action::Today,
    });
    out
}
pub fn tasks(events: &[Event]) -> Vec<&Event> {
    let mut out: Vec<_> = events.iter().filter(|e| e.task).collect();
    out.sort_by(|a, b| {
        (a.task_date.is_none(), a.task_date, a.start, &a.title).cmp(&(
            b.task_date.is_none(),
            b.task_date,
            b.start,
            &b.title,
        ))
    });
    out
}
pub fn sidebar_tasks(events: &[Event], day: NaiveDate) -> Vec<&Event> {
    tasks(events)
        .into_iter()
        .filter(|e| e.task_date.is_none() || e.task_date == Some(day))
        .collect()
}
pub fn task_page(events: &[Event], day: NaiveDate, page: usize, limit: usize) -> Vec<&Event> {
    let all = sidebar_tasks(events, day);
    let pages = all.len().div_ceil(limit).max(1);
    all.into_iter()
        .skip((page % pages) * limit)
        .take(limit)
        .collect()
}
// Calendar preview rows. Every part of a cell selects its day; only the sidebar toggles tasks.
fn cell_rows(events: &[Event], day: NaiveDate, tz: chrono_tz::Tz) -> Vec<&Event> {
    let agenda = day_events(events, day, tz);
    let capacity = if agenda
        .iter()
        .any(|e| e.all_day && e.production_day.is_none())
    {
        2
    } else {
        3
    };
    let timed: Vec<_> = agenda.into_iter().filter(|e| !e.all_day).collect();
    let mut tasks: Vec<_> = events
        .iter()
        .filter(|e| e.task && e.task_date == Some(day))
        .collect();
    tasks.sort_by(|a, b| (a.start, &a.title).cmp(&(b.start, &b.title)));
    let mut rows: Vec<_> = tasks
        .into_iter()
        .take(capacity - usize::from(!timed.is_empty()))
        .collect();
    rows.extend(timed.into_iter().take(capacity - rows.len()));
    rows
}
pub fn day_events(events: &[Event], day: NaiveDate, tz: chrono_tz::Tz) -> Vec<&Event> {
    let from = tz
        .from_local_datetime(&day.and_hms_opt(0, 0, 0).unwrap())
        .earliest()
        .unwrap()
        .with_timezone(&Utc);
    let to = tz
        .from_local_datetime(&day.succ_opt().unwrap().and_hms_opt(0, 0, 0).unwrap())
        .earliest()
        .unwrap()
        .with_timezone(&Utc);
    let mut out: Vec<_> = events
        .iter()
        .filter(|e| {
            !e.task && e.start < to && (e.end > from || (e.start == e.end && e.start >= from))
        })
        .collect();
    out.sort_by(|a, b| (!a.all_day, a.start, &a.title).cmp(&(!b.all_day, b.start, &b.title)));
    out
}
#[derive(Debug, Clone)]
pub struct SidebarLayout {
    pub task_header: i32,
    pub task_top: i32,
    pub task_limit: usize,
    pub agenda_end: i32,
    pub agenda_nav: i32,
}
pub const AGENDA_TITLE_LINES: usize = 5;
pub const AGENDA_LINE_HEIGHT: i32 = 28;
pub fn agenda_title_lines(event: &Event) -> Vec<String> {
    let width = if event.all_day { 24 } else { 20 };
    let mut lines = wrap(&event.title, width);
    if lines.len() > AGENDA_TITLE_LINES {
        lines.truncate(AGENDA_TITLE_LINES);
        let last = lines.last_mut().unwrap();
        *last = last
            .chars()
            .take(width - 1)
            .collect::<String>()
            .trim_end()
            .to_owned()
            + "…";
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}
fn agenda_row_height(event: &Event) -> i32 {
    (if event.all_day { 90 } else { 58 })
        + (agenda_title_lines(event).len() as i32 - 1) * AGENDA_LINE_HEIGHT
}
pub fn sidebar_layout(events: &[Event], day: NaiveDate, tz: chrono_tz::Tz) -> SidebarLayout {
    let mut end = 128;
    let mut timed = false;
    for e in day_events(events, day, tz) {
        end += agenda_row_height(e);
        if !e.all_day && !timed {
            end += 64;
            timed = true;
        }
    }
    // Quiet day: tasks use about half the usable panel. Busy day: events may
    // grow to three quarters, keeping at least two comfortable task rows.
    let paging = end + 12 > 1000;
    let task_header = if paging {
        1000
    } else {
        (end + 12).clamp(660, 1000)
    };
    let task_top = task_header + 100;
    SidebarLayout {
        task_header,
        task_top,
        task_limit: ((1320 - task_top) / TASK_HEIGHT).max(1) as usize,
        agenda_end: task_header - if paging { 90 } else { 12 },
        agenda_nav: task_header - 80,
    }
}
pub fn paging_controls(top: i32, previous: Action, next: Action, label: &str) -> Vec<Control> {
    vec![
        Control {
            label: format!("Предыдущие {label}"),
            x: 1638,
            y: top,
            width: 74,
            height: 64,
            action: previous,
        },
        Control {
            label: format!("Следующие {label}"),
            x: 1764,
            y: top,
            width: 74,
            height: 64,
            action: next,
        },
    ]
}
#[derive(Clone)]
pub struct AgendaRow<'a> {
    pub event: &'a Event,
    pub y: i32,
    pub heading: bool,
    pub height: i32,
    pub lines: Vec<String>,
}
impl AgendaRow<'_> {
    pub fn top(&self) -> i32 {
        if self.event.all_day {
            self.y
        } else {
            self.y - 28
        }
    }
    pub fn hit_height(&self) -> i32 {
        self.height - if self.event.all_day { 22 } else { 8 }
    }
}
pub fn agenda_pages<'a>(
    events: &'a [Event],
    day: NaiveDate,
    tz: chrono_tz::Tz,
) -> Vec<Vec<AgendaRow<'a>>> {
    let layout = sidebar_layout(events, day, tz);
    let mut pages = vec![Vec::new()];
    let mut y = 128;
    let mut timed_heading = false;
    for event in day_events(events, day, tz) {
        let mut heading = !event.all_day && !timed_heading;
        let height = agenda_row_height(event);
        let mut needed = height + if heading { 64 } else { 0 };
        if y + needed > layout.agenda_end && !pages.last().unwrap().is_empty() {
            pages.push(Vec::new());
            y = 128;
            timed_heading = false;
            heading = !event.all_day;
            needed = height + if heading { 64 } else { 0 };
        }
        let row_y = if heading { y + 64 } else { y };
        pages.last_mut().unwrap().push(AgendaRow {
            event,
            y: row_y,
            heading,
            height,
            lines: agenda_title_lines(event),
        });
        if !event.all_day {
            timed_heading = true;
        }
        y += needed;
    }
    pages
}
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn short(s: &str, n: usize) -> String {
    let text: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        format!("{text}…")
    } else {
        text
    }
}
fn text(s: &mut String, x: i32, y: i32, size: i32, fill: &str, weight: i32, value: &str) {
    write!(s,r#"<text x="{x}" y="{y}" font-size="{size}" font-weight="{weight}" fill="{fill}">{}</text>"#,escape(value)).unwrap();
}
fn rect(s: &mut String, x: i32, y: i32, w: i32, h: i32, fill: &str) {
    write!(
        s,
        r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="5" fill="{fill}"/>"#
    )
    .unwrap();
}
fn rule(s: &mut String, x: i32, y: i32, w: i32) {
    write!(s, r##"<path d="M{x} {y}h{w}" stroke="#bbbbbb"/>"##).unwrap();
}
fn pager(s: &mut String, top: i32, page: usize, count: usize) {
    if count > 1 {
        for x in [1638, 1764] {
            rect(s, x, top, 74, 64, "#eeeeee");
            write!(s,r##"<rect x="{x}" y="{top}" width="74" height="64" rx="7" fill="none" stroke="#bbbbbb"/>"##).unwrap();
        }
        text(s, 1663, top + 44, 38, "#111111", 400, "‹");
        text(s, 1788, top + 44, 38, "#111111", 400, "›");
        write!(s,r##"<text x="1738" y="{}" text-anchor="middle" font-size="19" fill="#333333">{}/{}</text>"##,top+39,page%count+1,count).unwrap();
    }
}
pub fn svg(
    events: &[Event],
    today: NaiveDate,
    tz: chrono_tz::Tz,
    font: &str,
    error: bool,
    _demo: bool,
    state: &RenderState<'_>,
) -> String {
    let selected = state.selection.day.unwrap_or(today);
    let month = state.selection.month.unwrap_or(today);
    let mut s = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="1872" height="1404" viewBox="0 0 1872 1404"><rect width="1872" height="1404" fill="white"/><g font-family="{}">"#,
        escape(font)
    );
    s.push_str(r#"<g id="calendar-base">"#);
    rect(&mut s, 36, 20, 200, 60, "#eeeeee");
    write!(
        s,
        r##"<rect x="36" y="20" width="200" height="60" rx="6" fill="none" stroke="#bbbbbb"/>"##
    )
    .unwrap();
    text(&mut s, 54, 59, 25, "#111111", 700, "Обновить");
    text(
        &mut s,
        266,
        61,
        42,
        "#111111",
        700,
        &format!("{} {}", MONTHS[month.month0() as usize], month.year()),
    );
    text(
        &mut s,
        1100,
        59,
        22,
        "#222222",
        400,
        &format!("Сегодня · {}", today.format("%d.%m")),
    );
    for (i, label) in ["ПН", "ВТ", "СР", "ЧТ", "ПТ", "СБ", "ВС"]
        .iter()
        .enumerate()
    {
        let x = 36 + i as i32 * 199;
        if i >= 5 {
            rect(&mut s, x + 5, 81, 189, 44, "#111111");
            text(&mut s, x + 75, 112, 26, "white", 700, label);
        } else {
            text(&mut s, x + 14, 112, 26, "#111111", 700, label);
        }
    }
    let start = month_range(month).0;
    let days: Vec<_> = (0..42)
        .map(|i| {
            day_events(events, start + Duration::days(i), tz)
                .into_iter()
                .filter(|e| e.production_day.is_none())
                .collect::<Vec<_>>()
        })
        .collect();
    for i in 0..42 {
        let day = start + Duration::days(i as i64);
        let production = crate::production::day_kind(events, day, tz);
        let x = 36 + (i % 7) * 199;
        let y = 130 + (i / 7) * 198;
        let fill = if production.map_or(i % 7 >= 5, |kind| kind.is_day_off()) {
            "#dedede"
        } else if day.month() != month.month() {
            "#f6f6f6"
        } else {
            "white"
        };
        rect(&mut s, x, y, 199, 198, fill);
        write!(
            s,
            r##"<path d="M{x} {y}h199v198" stroke="#aaaaaa" stroke-width="1.5" fill="none"/>"##
        )
        .unwrap();
        if day == today {
            write!(
                s,
                r##"<circle cx="{}" cy="{}" r="32" fill="#000000"/>"##,
                x + 157,
                y + 38
            )
            .unwrap();
        }
        text(
            &mut s,
            x + 134,
            y + 52,
            42,
            if day == today {
                "white"
            } else if day.month() == month.month() {
                "#111111"
            } else {
                "#777777"
            },
            700,
            &format!("{:02}", day.day()),
        );
        if let Some(kind) = production.filter(|kind| *kind != crate::production::DayKind::DayOff) {
            let label = kind.label();
            if kind.is_day_off() {
                write!(
                    s,
                    r##"<rect x="{}" y="{}" width="119" height="30" rx="4" fill="#222222"/>"##,
                    x + 8,
                    y + 14
                )
                .unwrap();
            }
            text(
                &mut s,
                x + 13,
                y + 36,
                19,
                if kind.is_day_off() {
                    "white"
                } else {
                    "#111111"
                },
                700,
                label,
            );
        }
        write!(s,r#"<clipPath id="cell{i}"><rect x="{}" y="{}" width="183" height="132"/></clipPath><g clip-path="url(#cell{i})">"#,x+8,y+64).unwrap();
        let entries = &days[i as usize];
        let all_day = entries.first().filter(|e| e.all_day);
        let timed: Vec<_> = entries.iter().filter(|e| !e.all_day).collect();
        let dated_tasks = events
            .iter()
            .filter(|e| e.task && e.task_date == Some(day))
            .count();
        let rows = cell_rows(events, day, tz);
        if dated_tasks > 0 {
            let base = if all_day.is_some() { 106 } else { 66 };
            for (j, e) in rows.iter().enumerate() {
                let ey = y + base + j as i32 * 38;
                if e.task {
                    write!(s,r##"<circle cx="{}" cy="{}" r="13" fill="white" stroke="#111111" stroke-width="2"/>"##,x+23,ey+19).unwrap();
                    text(
                        &mut s,
                        x + 43,
                        ey + 26,
                        21,
                        "#111111",
                        700,
                        &short(&e.title, 12),
                    );
                } else {
                    text(
                        &mut s,
                        x + 9,
                        ey + 26,
                        20,
                        "#111111",
                        700,
                        &format!(
                            "{} {}",
                            e.start.with_timezone(&tz).format("%H:%M"),
                            short(&e.title, 8)
                        ),
                    );
                }
            }
        } else if timed.len() == 1 && all_day.is_none() {
            let e = timed[0];
            text(
                &mut s,
                x + 9,
                y + 82,
                22,
                "#111111",
                700,
                &e.start.with_timezone(&tz).format("%H:%M").to_string(),
            );
            text(
                &mut s,
                x + 9,
                y + 110,
                23,
                "#111111",
                700,
                &short(&e.title, 13),
            );
        } else {
            let base = if all_day.is_some() { 119 } else { 96 };
            for (j, e) in timed.iter().take(2).enumerate() {
                let ey = y + base + j as i32 * 31;
                text(
                    &mut s,
                    x + 9,
                    ey,
                    21,
                    "#111111",
                    700,
                    &e.start.with_timezone(&tz).format("%H:%M").to_string(),
                );
                text(&mut s, x + 75, ey, 21, "#111111", 700, &short(&e.title, 8));
            }
        }
        let shown = if dated_tasks > 0 {
            rows.len()
        } else {
            timed.len().min(2)
        } + usize::from(all_day.is_some());
        let total = entries.len() + dated_tasks;
        if total > shown {
            text(
                &mut s,
                x + 9,
                y + 190,
                18,
                "#333333",
                700,
                &format!("+ ещё {}", total - shown),
            );
        }
        s.push_str("</g>");
    }
    // One all-day preview lane; merge a multi-day event across adjacent cells,
    // split at week boundaries, and count overlapping extras in each day's +N.
    for row in 0..6 {
        let mut col = 0;
        while col < 7 {
            let i = row * 7 + col;
            let first = days[i].first().filter(|e| e.all_day).copied();
            if let Some(event) = first {
                let mut end = col + 1;
                while end < 7
                    && days[row * 7 + end]
                        .first()
                        .is_some_and(|e| e.all_day && std::ptr::eq(*e, event))
                {
                    end += 1;
                }
                let x = 44 + col as i32 * 199;
                let y = 200 + row as i32 * 198;
                let width = (end - col) as i32 * 199 - 16;
                write!(s, r##"<rect x="{x}" y="{y}" width="{width}" height="31" rx="5" fill="white" stroke="#333333" stroke-width="2"/>"##).unwrap();
                text(
                    &mut s,
                    x + 8,
                    y + 22,
                    20,
                    "#111111",
                    700,
                    &short(&event.title, ((width - 20) / 12) as usize),
                );
                col = end;
            } else {
                col += 1;
            }
        }
    }
    rule(&mut s, 36, 1318, 1393);
    write!(s, r##"<path d="M1455 30V1340" stroke="#bbbbbb"/>"##).unwrap();
    s.push_str(r#"</g><g id="calendar-overlay">"#);
    // Completion markers are overlays too: toggling a task must not reshape
    // every label in the month. The underlying title remains crisp and legible.
    for index in 0..42 {
        let day = start + Duration::days(index);
        let x = 36 + (index % 7) as i32 * 199;
        let y = 130 + (index / 7) as i32 * 198;
        let base = if day_events(events, day, tz)
            .iter()
            .any(|e| e.all_day && e.production_day.is_none())
        {
            106
        } else {
            66
        };
        for (row, event) in cell_rows(events, day, tz).iter().enumerate() {
            if event.task && state.completed.contains(&event.task_id()) {
                let ey = y + base + row as i32 * 38;
                write!(s,r##"<circle cx="{}" cy="{}" r="13" fill="#111111" stroke="#111111" stroke-width="2"/><path d="M{} {}l6 6 11-13" stroke="white" stroke-width="3" fill="none"/>"##,x+23,ey+19,x+14,ey+19).unwrap();
                rule(&mut s, x + 42, ey + 19, 143);
            }
        }
    }
    let selected_index = (selected - start).num_days();
    if (0..42).contains(&selected_index) {
        let x = 36 + (selected_index % 7) * 199;
        let y = 130 + (selected_index / 7) * 198;
        write!(s,r##"<rect x="{}" y="{}" width="195" height="194" rx="4" fill="none" stroke="#111111" stroke-width="2"/>"##,x+2,y+2).unwrap();
    }

    if let Some(event) = state
        .selection
        .detail
        .as_ref()
        .and_then(|id| events.iter().find(|e| e.detail_id() == *id))
    {
        detail_panel(&mut s, event, tz, state);
    } else {
        let weekdays = [
            "Понедельник",
            "Вторник",
            "Среда",
            "Четверг",
            "Пятница",
            "Суббота",
            "Воскресенье",
        ];
        text(
            &mut s,
            1490,
            60,
            27,
            "#111111",
            700,
            &format!(
                "{}, {} {}",
                weekdays[selected.weekday().num_days_from_monday() as usize],
                selected.day(),
                MONTH_GENITIVE[selected.month0() as usize]
            ),
        );
        let entries = day_events(events, selected, tz);
        let all = entries.iter().filter(|e| e.all_day).count();
        text(
            &mut s,
            1490,
            98,
            20,
            "#333333",
            400,
            &format!("{} по времени · {} весь день", entries.len() - all, all),
        );
        let layout = sidebar_layout(events, selected, tz);
        let pages = agenda_pages(events, selected, tz);
        let page = state.selection.agenda_page % pages.len();
        for (row_index, row) in pages[page].iter().enumerate() {
            let e = row.event;
            if e.all_day {
                rect(&mut s, 1490, row.y, 338, row.hit_height(), "#dddddd");
                write!(s,r#"<clipPath id="agenda-title-{row_index}"><rect x="1503" y="{}" width="312" height="{}"/></clipPath><g clip-path="url(#agenda-title-{row_index})">"#,row.y+3,row.lines.len() as i32*AGENDA_LINE_HEIGHT+2).unwrap();
                for (i, line) in row.lines.iter().enumerate() {
                    text(
                        &mut s,
                        1503,
                        row.y + 27 + i as i32 * AGENDA_LINE_HEIGHT,
                        23,
                        "#111111",
                        700,
                        line,
                    );
                }
                s.push_str("</g>");
                text(
                    &mut s,
                    1503,
                    row.y + 52 + (row.lines.len() as i32 - 1) * AGENDA_LINE_HEIGHT,
                    19,
                    "#222222",
                    400,
                    "ВЕСЬ ДЕНЬ",
                );
            } else {
                if row.heading {
                    text(&mut s, 1490, row.y - 44, 20, "#333333", 700, "РАСПИСАНИЕ");
                }
                text(
                    &mut s,
                    1490,
                    row.y,
                    24,
                    "#111111",
                    700,
                    &e.start.with_timezone(&tz).format("%H:%M").to_string(),
                );
                write!(s,r#"<clipPath id="agenda-title-{row_index}"><rect x="1578" y="{}" width="250" height="{}"/></clipPath><g clip-path="url(#agenda-title-{row_index})">"#,row.y-23,row.lines.len() as i32*AGENDA_LINE_HEIGHT+2).unwrap();
                for (i, line) in row.lines.iter().enumerate() {
                    text(
                        &mut s,
                        1578,
                        row.y + i as i32 * AGENDA_LINE_HEIGHT,
                        22,
                        "#111111",
                        400,
                        line,
                    );
                }
                s.push_str("</g>");
                rule(
                    &mut s,
                    1490,
                    row.y + 19 + (row.lines.len() as i32 - 1) * AGENDA_LINE_HEIGHT,
                    338,
                );
            }
        }
        if entries.is_empty() {
            text(&mut s, 1490, 170, 24, "#333333", 400, "Нет событий");
        }
        if pages.len() > 1 {
            text(
                &mut s,
                1490,
                layout.agenda_nav + 40,
                22,
                "#333333",
                400,
                "События",
            );
        }
        pager(&mut s, layout.agenda_nav, page, pages.len());
        text(
            &mut s,
            1490,
            layout.task_header + 42,
            28,
            "#111111",
            700,
            "Задачи",
        );
        text(
            &mut s,
            1490,
            layout.task_header + 88,
            18,
            "#333333",
            400,
            "На выбранный день и без даты",
        );
        let task_events: Vec<_> = events
            .iter()
            .filter(|e| !e.task || !state.hidden_tasks.contains(&e.task_id()))
            .cloned()
            .collect();
        let task_count = sidebar_tasks(&task_events, selected).len();
        pager(
            &mut s,
            layout.task_header,
            state.selection.task_page,
            task_count.div_ceil(layout.task_limit).max(1),
        );
        for (i, e) in task_page(
            &task_events,
            selected,
            state.selection.task_page,
            layout.task_limit,
        )
        .iter()
        .enumerate()
        {
            let y = layout.task_top + i as i32 * TASK_HEIGHT;
            let done = state.completed.contains(&e.task_id());
            rect(
                &mut s,
                1490,
                y,
                58,
                58,
                if done { "#111111" } else { "white" },
            );
            write!(s,r##"<rect x="1490" y="{y}" width="58" height="58" rx="12" fill="none" stroke="#444444" stroke-width="3"/>"##).unwrap();
            if done {
                write!(s,r#"<path d="M1504 {}l10 11 20-25" stroke="white" stroke-width="5" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,y+29).unwrap();
            }
            let mut rows = vec![String::new()];
            for word in e.title.split_whitespace() {
                let line = rows.last_mut().unwrap();
                if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > 16 {
                    rows.push(word.into());
                } else {
                    if !line.is_empty() {
                        line.push(' ');
                    }
                    line.push_str(word);
                }
            }
            for (j, line) in rows.iter().take(2).enumerate() {
                text(
                    &mut s,
                    1568,
                    y + 25 + j as i32 * 30,
                    25,
                    if done { "#555555" } else { "#111111" },
                    700,
                    &short(line, 17),
                );
                if done {
                    write!(
                        s,
                        r##"<path d="M1568 {}h240" stroke="#666666" stroke-width="2"/>"##,
                        y + 17 + j as i32 * 30
                    )
                    .unwrap();
                }
            }
            text(
                &mut s,
                1568,
                y + 82,
                18,
                "#333333",
                400,
                &e.task_date
                    .map(|d| d.format("%d.%m").to_string())
                    .unwrap_or_else(|| "Без даты".into()),
            );
            rule(&mut s, 1490, y + 96, 338);
        }
        if task_count == 0 {
            text(
                &mut s,
                1490,
                layout.task_top + 35,
                24,
                "#333333",
                400,
                "Нет открытых задач",
            );
        }
    }
    text(
        &mut s,
        48,
        1372,
        20,
        "#333333",
        400,
        if state.pending_tasks > 0 {
            "Изменения задач ожидают синхронизации"
        } else if error {
            "Нет связи · показаны сохранённые события"
        } else {
            "Нажми на дату для просмотра дня"
        },
    );
    s.push_str("</g></g></svg>");
    s
}
pub const DETAIL_LINES: usize = 28;
pub fn wrap(value: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in value.split('\n') {
        if paragraph.trim().is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let letters: Vec<char> = word.chars().collect();
            for chunk in letters.chunks(width) {
                let part: String = chunk.iter().collect();
                if !line.is_empty() && line.chars().count() + 1 + part.chars().count() > width {
                    lines.push(std::mem::take(&mut line));
                }
                if !line.is_empty() {
                    line.push(' ');
                }
                line.push_str(&part);
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
    }
    lines
}
pub fn detail_lines(event: &Event, tz: chrono_tz::Tz) -> Vec<String> {
    let mut out = wrap(&event.title, 22);
    out.push(String::new());
    out.extend(wrap(&format!("Календарь: {}", event.calendar), 25));
    let when = if event.task {
        if event.all_day {
            format!(
                "Срок: {}",
                event.start.with_timezone(&tz).format("%d.%m.%Y")
            )
        } else {
            format!(
                "Срок: {}",
                event.start.with_timezone(&tz).format("%d.%m.%Y %H:%M")
            )
        }
    } else if event.all_day {
        format!(
            "{} · весь день",
            event.start.with_timezone(&tz).format("%d.%m.%Y")
        )
    } else {
        format!(
            "{}  {}–{}",
            event.start.with_timezone(&tz).format("%d.%m.%Y"),
            event.start.with_timezone(&tz).format("%H:%M"),
            event.end.with_timezone(&tz).format("%H:%M")
        )
    };
    out.extend(wrap(&when, 25));
    if !event.location.is_empty() {
        out.extend(wrap(&format!("Место: {}", event.location), 25));
    }
    out.push(String::new());
    out.push("Описание".into());
    out.extend(wrap(
        if event.description.trim().is_empty() {
            "Описание не добавлено"
        } else {
            &event.description
        },
        25,
    ));
    out
}
fn detail_panel(s: &mut String, event: &Event, tz: chrono_tz::Tz, state: &RenderState<'_>) {
    rect(s, 1490, 25, 338, 65, "#eeeeee");
    text(s, 1510, 68, 28, "#111111", 700, "‹ Назад к списку");
    let lines = detail_lines(event, tz);
    let pages = lines.len().div_ceil(DETAIL_LINES).max(1);
    let page = state.selection.detail_page % pages;
    // Clip only the right panel; long text is available on following pages.
    s.push_str(r#"<clipPath id="detail"><rect x="1490" y="115" width="338" height="1040"/></clipPath><g clip-path="url(#detail)">"#);
    for (i, line) in lines
        .iter()
        .skip(page * DETAIL_LINES)
        .take(DETAIL_LINES)
        .enumerate()
    {
        let is_title = page == 0 && i < wrap(&event.title, 22).len();
        text(
            s,
            1490,
            146 + i as i32 * 35,
            if is_title { 27 } else { 25 },
            "#111111",
            if is_title { 700 } else { 400 },
            line,
        );
    }
    s.push_str("</g>");
    if event.task {
        let done = state.completed.contains(&event.task_id());
        rect(
            s,
            1490,
            1195,
            338,
            70,
            if done { "#111111" } else { "#eeeeee" },
        );
        text(
            s,
            1504,
            1239,
            23,
            if done { "white" } else { "#111111" },
            700,
            if done {
                "✓ Выполнено · отменить"
            } else {
                "□ Отметить выполненной"
            },
        );
    }
    if pages > 1 {
        text(s, 1505, 1320, 32, "#111111", 700, "‹");
        text(
            s,
            1610,
            1318,
            22,
            "#333333",
            400,
            &format!("{}/{}", page + 1, pages),
        );
        text(s, 1790, 1320, 32, "#111111", 700, "›");
    }
}
pub fn png(svg: &str, font_path: &std::path::Path) -> Result<Vec<u8>> {
    Ok(png_and_pixels(svg, font_path)?.0)
}
pub fn png_and_pixels(svg: &str, font_path: &std::path::Path) -> Result<(Vec<u8>, Vec<u8>)> {
    let rgba = raster(svg, font_path)?;
    let pixmap = resvg::tiny_skia::Pixmap::from_vec(
        rgba.clone(),
        resvg::tiny_skia::IntSize::from_wh(1872, 1404).unwrap(),
    )
    .context("Restore image")?;
    Ok((pixmap.encode_png()?, rgba))
}
fn raster_options(font_path: &std::path::Path) -> Result<resvg::usvg::Options<'static>> {
    use std::sync::{Arc, Mutex, OnceLock};
    type FontCache = Option<(std::path::PathBuf, Arc<resvg::usvg::fontdb::Database>)>;
    static FONTS: OnceLock<Mutex<FontCache>> = OnceLock::new();
    let fontdb = {
        let mut cache = FONTS
            .get_or_init(|| Mutex::new(None))
            .lock()
            .map_err(|_| anyhow::anyhow!("Font cache unavailable"))?;
        if cache.as_ref().is_none_or(|(path, _)| path != font_path) {
            let mut options = resvg::usvg::Options::default();
            options
                .fontdb_mut()
                .load_font_file(font_path)
                .context("Load configured font with Cyrillic support")?;
            if let Some(parent) = font_path.parent() {
                for filename in ["Sans_Bold.ttf", "Arial Bold.ttf"] {
                    let bold = parent.join(filename);
                    if bold.exists() {
                        options.fontdb_mut().load_font_file(bold)?;
                    }
                }
            }
            *cache = Some((font_path.to_path_buf(), options.fontdb));
        }
        Arc::clone(&cache.as_ref().unwrap().1)
    };
    Ok(resvg::usvg::Options {
        fontdb,
        ..resvg::usvg::Options::default()
    })
}
fn full_raster(svg: &str, options: &resvg::usvg::Options<'_>) -> Result<resvg::tiny_skia::Pixmap> {
    let tree = resvg::usvg::Tree::from_str(svg, options)?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(1872, 1404).context("Allocate image")?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::default(),
        &mut pixmap.as_mut(),
    );
    Ok(pixmap)
}
// Keep exactly one static month layer. Selection, day agenda, task pages and
// status are transparent overlays, so browsing days does not reshape month text.
fn svg_layers(svg: &str) -> Option<(String, String)> {
    let (prefix, rest) = svg.split_once(r#"<g id="calendar-base">"#)?;
    let (base, overlay) = rest.split_once(r#"</g><g id="calendar-overlay">"#)?;
    let base = format!(r#"{prefix}<g id="calendar-base">{base}</g></g></svg>"#);
    let transparent_prefix =
        prefix.replacen(r#"<rect width="1872" height="1404" fill="white"/>"#, "", 1);
    let overlay = format!(r#"{transparent_prefix}<g id="calendar-overlay">{overlay}"#);
    Some((base, overlay))
}
pub fn raster(svg: &str, font_path: &std::path::Path) -> Result<Vec<u8>> {
    use std::sync::{Arc, Mutex, OnceLock};
    struct Layer {
        path: std::path::PathBuf,
        svg: String,
        pixels: Arc<resvg::tiny_skia::Pixmap>,
    }
    static BASE: OnceLock<Mutex<Option<Layer>>> = OnceLock::new();
    let options = raster_options(font_path)?;
    let Some((base, overlay)) = svg_layers(svg) else {
        return Ok(full_raster(svg, &options)?.take());
    };
    let pixels = {
        let mut cached = BASE
            .get_or_init(|| Mutex::new(None))
            .lock()
            .map_err(|_| anyhow::anyhow!("Month cache unavailable"))?;
        if cached
            .as_ref()
            .is_none_or(|layer| layer.path != font_path || layer.svg != base)
        {
            let pixels = Arc::new(full_raster(&base, &options)?);
            *cached = Some(Layer {
                path: font_path.to_path_buf(),
                svg: base,
                pixels,
            });
        }
        Arc::clone(&cached.as_ref().unwrap().pixels)
    };
    let overlay = full_raster(&overlay, &options)?;
    let mut result = pixels.as_ref().clone();
    // ARM soft-float blending must stay integer-only. A general bitmap
    // transform/compositor needlessly runs floating-point work over the screen.
    for (dst, src) in result
        .data_mut()
        .chunks_exact_mut(4)
        .zip(overlay.data().chunks_exact(4))
    {
        match src[3] {
            0 => {}
            255 => dst.copy_from_slice(src),
            alpha => {
                let inverse = 255 - u32::from(alpha);
                for channel in 0..3 {
                    dst[channel] = (u32::from(src[channel])
                        + (u32::from(dst[channel]) * inverse + 127) / 255)
                        .min(255) as u8;
                }
                dst[3] = 255;
            }
        }
    }
    Ok(result.take())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cached_month_is_independent_of_selection_and_composites_like_full_render() {
        let config = crate::test_config();
        let today = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let events = crate::demo_events(today, config.timezone);
        let completed = BTreeSet::new();
        let hidden = BTreeSet::new();
        let first = Selection::default();
        let second = Selection {
            day: NaiveDate::from_ymd_opt(2026, 10, 16),
            ..Selection::default()
        };
        let image = |selection: &Selection, marks: &BTreeSet<String>| {
            svg(
                &events,
                today,
                config.timezone,
                &config.font_family,
                false,
                true,
                &RenderState {
                    completed: marks,
                    hidden_tasks: &hidden,
                    pending_tasks: 0,
                    selection,
                },
            )
        };
        let a = image(&first, &completed);
        let b = image(&second, &completed);
        let (base_a, overlay_a) = svg_layers(&a).unwrap();
        let (base_b, overlay_b) = svg_layers(&b).unwrap();
        assert_eq!(base_a, base_b);
        assert_ne!(overlay_a, overlay_b);
        let options = raster_options(&config.font_path).unwrap();
        for svg in [&a, &b] {
            let cached = raster(svg, &config.font_path).unwrap();
            let full = full_raster(svg, &options).unwrap();
            let max_delta = cached
                .iter()
                .zip(full.data())
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap();
            assert!(
                max_delta <= 1,
                "Layer composition changed pixels by {max_delta}"
            );
            assert!(cached.chunks_exact(4).all(|pixel| pixel[3] == 255));
        }
        let task = events
            .iter()
            .find(|e| e.task && e.task_date.is_some())
            .unwrap();
        let marks = BTreeSet::from([task.task_id()]);
        let changed = image(&first, &marks);
        assert_eq!(base_a, svg_layers(&changed).unwrap().0);
        assert_ne!(overlay_a, svg_layers(&changed).unwrap().1);
        let cached = raster(&changed, &config.font_path).unwrap();
        let full = full_raster(&changed, &options).unwrap();
        let max_delta = cached
            .iter()
            .zip(full.data())
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        // Overlapping antialiased marker edges can round by 2/255 when
        // composited in two stages; larger changes would indicate a regression.
        let changed_pixels = cached
            .chunks_exact(4)
            .zip(full.data().chunks_exact(4))
            .filter(|(a, b)| a.iter().zip(b.iter()).any(|(a, b)| a.abs_diff(*b) > 1))
            .count();
        assert!(
            max_delta <= 2,
            "Completed marker delta {max_delta} in {changed_pixels} pixels"
        );
        let next = Selection {
            month: NaiveDate::from_ymd_opt(2026, 11, 1),
            ..Selection::default()
        };
        assert_ne!(base_a, svg_layers(&image(&next, &completed)).unwrap().0);
    }
    #[test]
    fn long_agenda_titles_wrap_to_five_lines_and_pages_do_not_overlap() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let tz = chrono_tz::Asia::Novosibirsk;
        let mut template = crate::demo_events(day, tz)
            .into_iter()
            .find(|e| !e.task && !e.all_day && e.start.with_timezone(&tz).date_naive() == day)
            .unwrap();
        template.title =
            "Обсуждение планов команды и подготовка большого проекта к следующему рабочему месяцу"
                .into();
        let lines = agenda_title_lines(&template);
        assert_eq!(lines.len(), 5);
        assert_eq!(lines.join(" "), template.title);
        let original = template.title.clone();
        template.title = original.repeat(5);
        let lines = agenda_title_lines(&template);
        assert_eq!(lines.len(), 5);
        assert!(lines[4].ends_with('…'));
        template.title = original;
        let mut events = Vec::new();
        for i in 0..20 {
            let mut event = template.clone();
            event.uid = format!("long-{i}");
            event.start = template.start + Duration::minutes(i);
            event.end = event.start + Duration::minutes(1);
            events.push(event);
        }
        let mut all_day = template.clone();
        all_day.uid = "all-day-long".into();
        all_day.all_day = true;
        events.push(all_day);
        let pages = agenda_pages(&events, day, tz);
        assert!(pages.len() > 1);
        assert_eq!(pages.iter().map(Vec::len).sum::<usize>(), events.len());
        let layout = sidebar_layout(&events, day, tz);
        for page in &pages {
            assert!(page.iter().all(
                |row| row.lines.len() <= 5 && row.top() + row.hit_height() <= layout.agenda_end
            ));
            assert!(page
                .windows(2)
                .all(|pair| pair[0].top() + pair[0].hit_height() <= pair[1].top()));
        }
    }
    #[test]
    fn detail_pages_preserve_long_description_and_month() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let tz = chrono_tz::Asia::Novosibirsk;
        let mut events = crate::demo_events(day, tz);
        let task = events.iter_mut().find(|e| e.task).unwrap();
        task.description = "Подробное описание задачи.\n\n".repeat(80);
        let id = task.detail_id();
        assert!(detail_lines(task, tz).len() > DETAIL_LINES * 2);
        let completed = BTreeSet::new();
        let normal = Selection::default();
        let details = Selection {
            detail: Some(id),
            ..Selection::default()
        };
        let a = svg(
            &events,
            day,
            tz,
            "Arial",
            false,
            true,
            &RenderState {
                completed: &completed,
                hidden_tasks: &BTreeSet::new(),
                pending_tasks: 0,
                selection: &normal,
            },
        );
        let b = svg(
            &events,
            day,
            tz,
            "Arial",
            false,
            true,
            &RenderState {
                completed: &completed,
                hidden_tasks: &BTreeSet::new(),
                pending_tasks: 0,
                selection: &details,
            },
        );
        assert_eq!(
            a.split("M1455 30V1340").next(),
            b.split("M1455 30V1340").next()
        );
        assert!(b.contains("Назад к списку"));
        assert!(roxmltree::Document::parse(&b).is_ok());
    }
    #[test]
    fn month_grid_and_escaping() {
        let d = NaiveDate::from_ymd_opt(2026, 2, 1).unwrap();
        assert_eq!(
            month_range(d).0,
            NaiveDate::from_ymd_opt(2026, 1, 26).unwrap()
        );
        let completed = BTreeSet::new();
        let selection = Selection::default();
        let svg = svg(
            &[],
            d,
            chrono_tz::Asia::Novosibirsk,
            "A&B",
            false,
            true,
            &RenderState {
                completed: &completed,
                hidden_tasks: &BTreeSet::new(),
                pending_tasks: 0,
                selection: &selection,
            },
        );
        assert!(svg.contains("Февраль"));
        assert!(svg.contains("A&amp;B"));
        assert!(roxmltree::Document::parse(&svg).is_ok());
    }
    #[test]
    fn adaptive_layout_keeps_tasks_and_navigation_inside_screen() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let tz = chrono_tz::Asia::Novosibirsk;
        let demo = crate::demo_events(day, tz);
        let template = demo.iter().find(|e| !e.task && !e.all_day).unwrap().clone();
        for count in 0..50 {
            let mut events: Vec<_> = demo.iter().filter(|e| e.task).cloned().collect();
            for i in 0..count {
                let mut e = template.clone();
                e.start = tz
                    .from_local_datetime(&day.and_hms_opt(0, 0, 0).unwrap())
                    .single()
                    .unwrap()
                    .with_timezone(&Utc)
                    + Duration::minutes(i * 20);
                e.end = e.start + Duration::minutes(10);
                events.push(e);
            }
            let layout = sidebar_layout(&events, day, tz);
            assert!((660..=1000).contains(&layout.task_header));
            assert!((2..=5).contains(&layout.task_limit));
            assert!(layout.task_top + layout.task_limit as i32 * TASK_HEIGHT <= 1320);
            let pages = agenda_pages(&events, day, tz);
            assert_eq!(pages.iter().map(Vec::len).sum::<usize>(), count as usize);
            assert!(pages
                .iter()
                .flatten()
                .all(|r| r.y + 19 < layout.task_header));
            if count <= 10 {
                assert_eq!(pages.len(), 1);
            }
            if count > 20 {
                assert!(pages.len() > 1);
                assert_eq!(layout.task_limit, 2);
            }
        }
    }
    #[test]
    fn busy_schedule_grows_before_paginating() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let events = crate::demo_events(day, chrono_tz::Asia::Novosibirsk);
        let pages = agenda_pages(&events, day, chrono_tz::Asia::Novosibirsk);
        assert_eq!(pages.len(), 1);
        assert_eq!(pages.iter().map(Vec::len).sum::<usize>(), 11);
        assert!(pages
            .iter()
            .flatten()
            .all(|r| r.y < sidebar_layout(&events, day, chrono_tz::Asia::Novosibirsk).agenda_end));
    }
    #[test]
    fn pagination_never_drops_busy_day_events() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let mut events = crate::demo_events(day, chrono_tz::Asia::Novosibirsk);
        let template = events.iter().find(|e| !e.task).unwrap().clone();
        for i in 0..30 {
            let mut e = template.clone();
            e.start =
                Utc.from_utc_datetime(&day.and_hms_opt(0, 0, 0).unwrap()) + Duration::minutes(i);
            e.end = e.start + Duration::minutes(1);
            e.all_day = i < 5;
            e.uid = format!("test-{i}");
            events.push(e);
        }
        let expected = day_events(&events, day, chrono_tz::UTC).len();
        let pages = agenda_pages(&events, day, chrono_tz::UTC);
        assert!(pages.len() > 1);
        assert_eq!(pages.iter().map(Vec::len).sum::<usize>(), expected);
        assert!(pages.iter().flatten().all(|row| row.y < 1000));
    }
}
