//! Production-day metadata comes only from an explicitly designated subscription.
//! No holiday dates or annual transfer rules are embedded in the application.
use crate::{Config, Event};
use chrono::{NaiveDate, TimeZone};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DayKind {
    DayOff,
    Holiday,
    Shortened,
    Working,
}
impl DayKind {
    pub fn is_day_off(self) -> bool {
        matches!(self, Self::DayOff | Self::Holiday)
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::DayOff => "выходной",
            Self::Holiday => "праздник",
            Self::Shortened => "сокращён.",
            Self::Working => "рабочий",
        }
    }
}
pub fn classify(enabled: bool, task: bool, all_day: bool, title: &str) -> Option<DayKind> {
    if !enabled || task || !all_day || title.trim().is_empty() {
        return None;
    }
    let title = title.to_lowercase().replace('ё', "е");
    Some(if title.contains("сокращен") {
        DayKind::Shortened
    } else if title.contains("нерабоч") {
        DayKind::DayOff
    } else if title.contains("рабочий") || title.contains("рабочая") {
        DayKind::Working
    } else if title.contains("выходн") || title.contains("перенос") {
        DayKind::DayOff
    } else {
        // A named all-day entry in this dedicated feed is a public holiday.
        DayKind::Holiday
    })
}
pub fn apply_roles(events: &mut [Event], config: &Config) {
    for event in events {
        let enabled = config.calendars.iter().any(|source| {
            source.production_calendar
                && source.name == event.calendar
                && source.url == event.collection_url
        });
        event.production_day = classify(enabled, event.task, event.all_day, &event.title);
    }
}
pub fn day_kind(events: &[Event], day: NaiveDate, tz: chrono_tz::Tz) -> Option<DayKind> {
    let start = tz
        .from_local_datetime(&day.and_hms_opt(0, 0, 0)?)
        .single()?;
    let end = tz
        .from_local_datetime(&day.succ_opt()?.and_hms_opt(0, 0, 0)?)
        .single()?;
    events
        .iter()
        .filter(|e| e.start < end && e.end > start)
        .filter_map(|e| e.production_day)
        .max_by_key(|k| match k {
            DayKind::DayOff => 0,
            DayKind::Holiday => 1,
            DayKind::Working => 2,
            DayKind::Shortened => 3,
        })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_designated_all_day_events_classify() {
        for (title, expected) in [
            ("Выходные", DayKind::DayOff),
            ("Перенос выходного дня", DayKind::DayOff),
            ("День народного единства", DayKind::Holiday),
            ("* Сокращённый день", DayKind::Shortened),
            ("Рабочая суббота", DayKind::Working),
            ("Рабочий день (перенос выходного)", DayKind::Working),
        ] {
            assert_eq!(classify(true, false, true, title), Some(expected));
            assert_eq!(classify(false, false, true, title), None);
            assert_eq!(classify(true, true, true, title), None);
            assert_eq!(classify(true, false, false, title), None);
        }
    }
    #[test]
    fn subscription_overrides_weekend_and_uses_exclusive_local_end() {
        let tz = chrono_tz::Asia::Novosibirsk;
        let d = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let mut events = crate::demo_events(d, tz);
        events.truncate(1);
        let e = &mut events[0];
        e.production_day = Some(DayKind::Shortened);
        e.start = tz
            .from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
            .single()
            .unwrap()
            .with_timezone(&chrono::Utc);
        e.end = tz
            .from_local_datetime(&d.succ_opt().unwrap().and_hms_opt(0, 0, 0).unwrap())
            .single()
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert_eq!(day_kind(&events, d, tz), Some(DayKind::Shortened));
        assert!(!day_kind(&events, d, tz).unwrap().is_day_off());
        assert_eq!(day_kind(&events, d.succ_opt().unwrap(), tz), None);
    }
    #[test]
    fn roles_survive_cache_and_render_working_weekend_without_day_off_fill() {
        let mut config = crate::test_config();
        let source = &mut config.calendars[0];
        source.production_calendar = true;
        let d = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let mut events = crate::demo_events(d, config.timezone);
        events.truncate(1);
        let e = &mut events[0];
        e.title = "Рабочая суббота".into();
        e.calendar = config.calendars[0].name.clone();
        e.collection_url = config.calendars[0].url.clone();
        e.task = false;
        e.all_day = true;
        e.start = config
            .timezone
            .from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
            .single()
            .unwrap()
            .with_timezone(&chrono::Utc);
        e.end = e.start + chrono::Duration::days(1);
        apply_roles(&mut events, &config);
        assert_eq!(events[0].production_day, Some(DayKind::Working));
        let json = serde_json::to_string(&events).unwrap();
        let cached: Vec<Event> = serde_json::from_str(&json).unwrap();
        assert_eq!(
            day_kind(&cached, d, config.timezone),
            Some(DayKind::Working)
        );
        let empty = std::collections::BTreeSet::new();
        let svg = crate::render::svg(
            &cached,
            d,
            config.timezone,
            &config.font_family,
            false,
            false,
            &crate::render::RenderState {
                completed: &empty,
                hidden_tasks: &empty,
                pending_tasks: 0,
                selection: &crate::render::Selection::default(),
            },
        );
        assert!(svg
            .contains(r##"<rect x="1031" y="922" width="199" height="198" rx="5" fill="white""##));
        assert!(svg.contains(">рабочий</text>"));
        if let Some(path) = std::env::var_os("PAPER_CALENDAR_PRODUCTION_PREVIEW") {
            std::fs::write(path, crate::render::png(&svg, &config.font_path).unwrap()).unwrap();
        }
        assert!(!svg.contains(">выходной</text>"));
        assert!(svg.contains(
            r##"<rect x="1230" y="922" width="199" height="198" rx="5" fill="#dedede""##
        ));
        config.calendars[0].production_calendar = false;
        apply_roles(&mut events, &config);
        assert_eq!(events[0].production_day, None);
    }
}
