mod caldav;
mod energy;
#[cfg(feature = "pocketbook")]
#[path = "native.rs"]
mod pocketbook;
mod production;
mod render;
mod task_sync;
mod todo;

use anyhow::{Context, Result};
use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::RwLock;

#[cfg(test)]
fn test_config() -> Config {
    let mut config: Config = serde_json::from_str(include_str!("../config.example.json")).unwrap();
    if let Some(path) = std::env::var_os("PAPER_CALENDAR_TEST_FONT") {
        config.font_path = path.into();
    } else if !config.font_path.exists() {
        config.font_path = PathBuf::from("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf");
        config.font_family = "DejaVu Sans".into();
    }
    config
}

fn default_web_enabled() -> bool {
    true
}

#[derive(Clone, Deserialize)]
pub struct Config {
    listen: String,
    #[serde(default = "default_web_enabled")]
    web_enabled: bool,
    pub timezone: chrono_tz::Tz,
    refresh_seconds: u64,
    cache_dir: PathBuf,
    pub font_path: PathBuf,
    pub font_family: String,
    pub calendars: Vec<Calendar>,
    #[serde(default)]
    pub power: energy::PowerConfig,
}
#[derive(Clone, Deserialize)]
pub struct Calendar {
    #[serde(default)]
    pub production_calendar: bool,
    pub name: String,
    pub url: String,
    pub username_env: String,
    pub password_env: String,
    #[serde(default)]
    pub tasks: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Event {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub production_day: Option<production::DayKind>,
    #[serde(default)]
    pub uid: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub location: String,
    pub calendar: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub all_day: bool,
    pub task: bool,
    #[serde(default)]
    pub task_date: Option<NaiveDate>,
    #[serde(default)]
    pub collection_url: String,
    #[serde(default)]
    pub resource_url: String,
    #[serde(default)]
    pub server_completed: bool,
}
impl Event {
    pub fn detail_id(&self) -> String {
        if self.task {
            self.task_id()
        } else {
            format!("{}-{}", self.task_id(), self.start.timestamp())
        }
    }
    pub fn task_id(&self) -> String {
        let identity = if self.uid.is_empty() {
            &self.title
        } else {
            &self.uid
        };
        format!(
            "{:x}",
            Sha256::digest(format!("{}\0{}", self.calendar, identity).as_bytes())
        )
    }
}
#[derive(Clone)]
struct View {
    events: Vec<Event>,
    today: NaiveDate,
    config: Config,
}
#[derive(Default, Clone, Serialize, Deserialize)]
struct Cache {
    events: Vec<Event>,
    last_success: Option<DateTime<Utc>>,
}
#[derive(Default)]
struct Screen {
    png: Vec<u8>,
    rgba: Vec<u8>,
    busy: bool,
    manual_refresh: bool,
    next_check: i64,
    idle_until: Option<i64>,
    network_request: i8,
    network_result: bool,
    wake: Arc<tokio::sync::Notify>,
    network_done: Arc<tokio::sync::Notify>,
    native: bool,
    metrics: energy::Metrics,
    etag: String,
    last_success: Option<DateTime<Utc>>,
    error: bool,
    demo: bool,
    completed: BTreeSet<String>,
    completed_until: BTreeMap<String, i64>,
    outbox: task_sync::Outbox,
    next_task_sync: i64,
    migrate_marks: bool,
    selection: render::Selection,
    view: Option<View>,
}
type Shared = Arc<RwLock<Screen>>;

pub fn month_range(today: NaiveDate) -> (NaiveDate, NaiveDate) {
    let first = today.with_day(1).unwrap();
    let start = first - Duration::days(first.weekday().num_days_from_monday() as i64);
    (start, start + Duration::days(42))
}
fn demo_description(title: &str) -> String {
    match title {
        "Заказать продукты"=>"Составить заказ на неделю.\n\nМолоко, яйца, хлеб, овощи и фрукты. Проверить, что осталось дома, и выбрать доставку на вечер.\n\nПеред оплатой проверить состав заказа и время доставки.".into(),
        "Встреча"=>"Обсудить планы и договориться о следующих шагах.\n\nПовестка:\n1. Текущие задачи и сроки.\n2. Вопросы, которые требуют решения.\n3. Ответственные и дальнейшие действия.\n\nПодготовить заметки и список вопросов до начала встречи.".into(),
        _=>format!("{title}\n\nЗдесь будет описание из календаря CalDAV: подробности, заметки и инструкции.\n\nНазвание открывает описание, а галочка отдельно отмечает выполнение задачи."),
    }
}
fn demo_events(today: NaiveDate, tz: chrono_tz::Tz) -> Vec<Event> {
    use chrono::TimeZone;
    let mut events: Vec<Event> = [
        (-23, 10, "Планы на неделю", false),
        (-18, 14, "Встреча по проекту", false),
        (-15, 9, "Стоматолог", false),
        (-10, 11, "Обсудить отпуск", false),
        (-5, 15, "Созвон с командой", false),
        (-2, 18, "Тренировка", false),
        (0, 10, "Встреча с друзьями", false),
        (2, 14, "Встреча по проекту", false),
        (4, 18, "Ужин с друзьями", false),
        (0, 12, "Заказать продукты", true),
        (1, 12, "Записаться к врачу", true),
        (3, 12, "Забрать посылку", true),
        (2, 12, "Полить цветы", true),
        (3, 13, "Оплатить интернет", true),
        (4, 12, "Разобрать документы", true),
        (5, 12, "Позвонить родителям", true),
    ]
    .into_iter()
    .map(|(day, hour, title, task)| {
        let start = tz
            .from_local_datetime(
                &(today + Duration::days(day))
                    .and_hms_opt(hour, 0, 0)
                    .unwrap(),
            )
            .single()
            .unwrap()
            .with_timezone(&Utc);
        Event {
            production_day: None,
            uid: format!("demo-{title}"),
            title: title.into(),
            description: demo_description(title),
            location: String::new(),
            calendar: "Личный".into(),
            start,
            end: start + Duration::hours(1),
            all_day: false,
            task,
            task_date: task.then_some(today + Duration::days(day)),
            collection_url: String::new(),
            resource_url: String::new(),
            server_completed: false,
        }
    })
    .collect();
    events.retain(|e| e.task || e.start.with_timezone(&tz).date_naive() != today);
    for (hour, title) in [
        (9, "Планы на неделю"),
        (11, "Встреча"),
        (12, "Обед"),
        (13, "Работа над проектом"),
        (14, "Звонок в сервис"),
        (15, "Проверить документы"),
        (16, "Покупки"),
        (17, "Тренировка"),
        (18, "Ужин"),
        (20, "Чтение"),
    ] {
        let start = tz
            .from_local_datetime(&today.and_hms_opt(hour, 0, 0).unwrap())
            .single()
            .unwrap()
            .with_timezone(&Utc);
        events.push(Event {
            production_day: None,
            uid: format!("demo-agenda-{hour}"),
            title: title.into(),
            description: demo_description(title),
            location: String::new(),
            calendar: "Личный".into(),
            start,
            end: start + Duration::minutes(45),
            all_day: false,
            task: false,
            task_date: None,
            collection_url: String::new(),
            resource_url: String::new(),
            server_completed: false,
        });
    }
    for (start_day, end_day, title) in [
        (today, today.succ_opt().unwrap(), "Подготовить отчёт"),
        (
            today.with_day(5).unwrap(),
            today.with_day(8).unwrap(),
            "Отпуск",
        ),
        (
            today.with_day(17).unwrap(),
            today.with_day(18).unwrap(),
            "День рождения",
        ),
    ] {
        let start = tz
            .from_local_datetime(&start_day.and_hms_opt(0, 0, 0).unwrap())
            .single()
            .unwrap()
            .with_timezone(&Utc);
        let end = tz
            .from_local_datetime(&end_day.and_hms_opt(0, 0, 0).unwrap())
            .single()
            .unwrap()
            .with_timezone(&Utc);
        events.push(Event {
            production_day: None,
            uid: format!("demo-allday-{title}"),
            title: title.into(),
            description: demo_description(title),
            location: String::new(),
            calendar: "Личный".into(),
            start,
            end,
            all_day: true,
            task: false,
            task_date: None,
            collection_url: String::new(),
            resource_url: String::new(),
            server_completed: false,
        });
    }
    events
}
fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}
fn midnight(config: &Config) -> i64 {
    use chrono::TimeZone;
    let tomorrow = Utc::now()
        .with_timezone(&config.timezone)
        .date_naive()
        .succ_opt()
        .unwrap();
    config
        .timezone
        .from_local_datetime(&tomorrow.and_hms_opt(0, 0, 0).unwrap())
        .earliest()
        .unwrap()
        .timestamp()
}
async fn radio(shared: &Shared, on: bool) -> bool {
    let done = {
        let mut s = shared.write().await;
        s.network_request = if on { 1 } else { -1 };
        s.network_done.clone()
    };
    if tokio::time::timeout(std::time::Duration::from_secs(45), done.notified())
        .await
        .is_err()
    {
        shared.write().await.network_request = 0;
        return false;
    }
    shared.read().await.network_result
}
fn save_energy_report(screen: &mut Screen) -> Result<()> {
    screen.metrics.last_report_utc = Some(Utc::now().to_rfc3339());
    if let Some(view) = &screen.view {
        atomic_write(
            &view.config.cache_dir.join("power-status.json"),
            &serde_json::to_vec_pretty(&screen.metrics)?,
        )?;
    }
    Ok(())
}
async fn poll(config: Config, shared: Shared, demo: bool) -> Result<()> {
    std::fs::create_dir_all(&config.cache_dir)?;
    let cache_path = config.cache_dir.join("events.json");
    let mut cache: Cache = match std::fs::read(&cache_path) {
        Ok(data) => serde_json::from_slice(&data).context("Invalid saved calendar cache")?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Cache::default(),
        Err(e) => return Err(e.into()),
    };
    production::apply_roles(&mut cache.events, &config);
    let mut last_attempt = 0i64;
    let mut fetched_month = None;
    let mut error = false;
    loop {
        let now = Utc::now().timestamp();
        let today = Utc::now().with_timezone(&config.timezone).date_naive();
        let (native, charging, wake, first, changed_day, manual, month, write_due) = {
            let mut s = shared.write().await;
            todo::expire_completed(&mut s, now)?;
            todo::return_home_if_idle(&mut s, now)?;
            let manual = std::mem::take(&mut s.manual_refresh);
            (
                s.native,
                s.metrics.charging,
                s.wake.clone(),
                s.view.is_none(),
                s.view.as_ref().is_some_and(|v| v.today != today),
                manual,
                s.selection.month.unwrap_or(today).with_day(1).unwrap(),
                !s.outbox.is_empty() && now >= s.next_task_sync,
            )
        };
        let interval = if native && !charging {
            config.power.battery_refresh_seconds
        } else {
            config.refresh_seconds
        };
        let due = first
            || manual
            || (!demo && write_due)
            || (!demo && (fetched_month != Some(month) || now >= last_attempt + interval as i64));
        if due || changed_day {
            {
                let mut s = shared.write().await;
                s.busy = true;
                if manual {
                    s.metrics.manual_refreshes += 1;
                }
            }
            if first && !demo && !cache.events.is_empty() {
                let mut s = shared.write().await;
                s.view = Some(View {
                    events: cache.events.clone(),
                    today,
                    config: config.clone(),
                });
                s.last_success = cache.last_success;
                todo::redraw(&mut s)?;
            }
            if demo {
                cache.events = demo_events(today, config.timezone);
            } else {
                let manage_wifi = native && config.power.wifi_on_demand;
                let attempted = shared.read().await.outbox.clone();
                let connected = !manage_wifi || radio(&shared, true).await;
                let result = if connected {
                    let client = reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(25))
                        .redirect(reqwest::redirect::Policy::none())
                        .build()?;
                    for (id, change) in attempted.clone() {
                        if task_sync::send(&client, &config, &change).await.is_ok() {
                            let mut s = shared.write().await;
                            if s.outbox.get(&id) == Some(&change) {
                                let previous = s.outbox.remove(&id).unwrap();
                                if task_sync::save(&config.cache_dir, &s.outbox).is_err() {
                                    s.outbox.insert(id, previous);
                                    eprintln!("Task confirmation could not be saved; update remains queued");
                                }
                            }
                        } else {
                            eprintln!(
                                "Task status update deferred; will retry on next synchronization"
                            );
                        }
                    }
                    caldav::fetch(&client, &config, month).await
                } else {
                    Err(anyhow::anyhow!("Wi-Fi unavailable"))
                };
                if manage_wifi {
                    radio(&shared, false).await;
                }
                last_attempt = Utc::now().timestamp();
                fetched_month = Some(month);
                let mut s = shared.write().await;
                s.metrics.sync_checks += 1;
                if !s.outbox.is_empty() {
                    let newer = s
                        .outbox
                        .iter()
                        .any(|(id, change)| attempted.get(id) != Some(change));
                    s.next_task_sync =
                        Utc::now().timestamp() + if newer { 3 } else { interval as i64 };
                }
                match result {
                    Ok(events) => {
                        error = false;
                        task_sync::reconcile(&mut s, &events, &config.cache_dir)?;
                        let changed = cache.events != events || cache.last_success.is_none();
                        cache = Cache {
                            events,
                            last_success: Some(Utc::now()),
                        };
                        if changed {
                            atomic_write(&cache_path, &serde_json::to_vec(&cache)?)?;
                        }
                    }
                    Err(_) => {
                        error = true;
                        eprintln!("CalDAV unavailable; retaining cached events");
                    }
                }
            }
            let mut s = shared.write().await;
            s.last_success = cache.last_success;
            s.error = error;
            s.demo = demo;
            s.view = Some(View {
                events: cache.events.clone(),
                today,
                config: config.clone(),
            });
            todo::redraw(&mut s)?;
            s.busy = false;
            save_energy_report(&mut s)?;
        }
        let now = Utc::now().timestamp();
        let next = if demo {
            midnight(&config)
        } else {
            (last_attempt + interval as i64).min(midnight(&config))
        };
        let next = {
            let mut s = shared.write().await;
            let next = todo::next_display_deadline(&s, next);
            let next = if !demo && !s.outbox.is_empty() {
                next.min(s.next_task_sync)
            } else {
                next
            };
            s.next_check = next;
            next
        };
        let delay = std::time::Duration::from_secs((next - now).max(1) as u64);
        // A wake from RTC must recheck wall time: CLOCK_MONOTONIC timers pause
        // during suspend, so a Tokio interval alone misses the sync deadline.
        tokio::select! {_=tokio::time::sleep(delay)=>{},_=wake.notified()=>{}}
    }
}
async fn frame(State(shared): State<Shared>, headers: HeaderMap) -> impl IntoResponse {
    let s = shared.read().await;
    if s.png.is_empty() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            HeaderMap::new(),
            Vec::new(),
        );
    }
    let mut response = HeaderMap::new();
    response.insert(header::ETAG, s.etag.parse().unwrap());
    response.insert(header::CACHE_CONTROL, "no-cache".parse().unwrap());
    response.insert(header::CONTENT_TYPE, "image/png".parse().unwrap());
    let matches = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',')
                .any(|tag| tag.trim().trim_start_matches("W/") == s.etag || tag.trim() == "*")
        });
    if matches {
        (StatusCode::NOT_MODIFIED, response, Vec::new())
    } else {
        (StatusCode::OK, response, s.png.clone())
    }
}
async fn status(State(shared): State<Shared>) -> impl IntoResponse {
    let s = shared.read().await;
    axum::Json(
        serde_json::json!({"ready": !s.rgba.is_empty(), "last_success": s.last_success, "sync_error": s.error, "demo": s.demo,"power": &s.metrics,"next_check":s.next_check,"busy":s.busy,"pending_task_updates":s.outbox.len()}),
    )
}
#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let config_path = args
        .windows(2)
        .find(|v| v[0] == "--config")
        .map(|v| v[1].as_str())
        .unwrap_or("config.json");
    let config: Config = serde_json::from_slice(
        &std::fs::read(config_path)
            .context("Read config; copy config.example.json to config.json")?,
    )?;
    anyhow::ensure!(
        config.refresh_seconds >= 30,
        "refresh_seconds must be at least 30"
    );
    anyhow::ensure!(
        config.power.battery_refresh_seconds >= 30 && config.power.idle_seconds <= 3600,
        "Invalid power configuration"
    );
    let demo = args.iter().any(|a| a == "--demo");
    anyhow::ensure!(
        demo || !config.calendars.is_empty(),
        "Configure at least one calendar"
    );
    if let Some(path) = args.windows(2).find(|v| v[0] == "--render").map(|v| &v[1]) {
        let today = Utc::now().with_timezone(&config.timezone).date_naive();
        let events = if demo {
            demo_events(today, config.timezone)
        } else {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(25))
                .redirect(reqwest::redirect::Policy::none())
                .build()?;
            caldav::fetch(&client, &config, today).await.map_err(|_| anyhow::anyhow!("CalDAV sync failed; check collection URL, environment credentials and REPORT/expand support"))?
        };
        let selection = render::Selection {
            detail: if args.iter().any(|a| a == "--detail-demo") {
                events.iter().find(|e| e.task).map(Event::detail_id)
            } else {
                None
            },
            ..render::Selection::default()
        };
        let svg = render::svg(
            &events,
            today,
            config.timezone,
            &config.font_family,
            false,
            demo,
            &render::RenderState {
                completed: &BTreeSet::new(),
                hidden_tasks: &BTreeSet::new(),
                pending_tasks: 0,
                selection: &selection,
            },
        );
        atomic_write(
            std::path::Path::new(path),
            &render::png(&svg, &config.font_path)?,
        )?;
        atomic_write(
            &std::path::Path::new(path).with_extension("svg"),
            svg.as_bytes(),
        )?;
        println!("Rendered {path}");
        return Ok(());
    }
    let native = args.iter().any(|a| a == "--inkview");
    let mut completed = todo::load(&config.cache_dir)?;
    let migrate_marks = !config.cache_dir.join("task-outbox.json").exists();
    let outbox = task_sync::load(&config.cache_dir)?;
    for (id, change) in &outbox {
        if change.completed {
            completed.insert(id.clone());
        } else {
            completed.remove(id);
        }
    }
    let shared = Arc::new(RwLock::new(Screen {
        completed,
        outbox,
        migrate_marks,
        native,
        ..Screen::default()
    }));
    let app = Router::new()
        .route("/", get(|| async { Html(include_str!("preview.html")) }))
        .route("/calendar.png", get(frame))
        .route("/status", get(status))
        .route("/tasks", get(todo::list))
        .route("/controls", get(todo::controls))
        .route("/tap", axum::routing::post(todo::handle_tap))
        .route("/tasks/complete", axum::routing::post(todo::complete))
        .with_state(shared.clone());
    let address = config.listen.parse()?;
    let web_enabled = config.web_enabled;
    let server = async move {
        if !web_enabled {
            return std::future::pending::<Result<()>>().await;
        }
        axum::Server::try_bind(&address)
            .context("Cannot start web preview")?
            .serve(app.into_make_service())
            .await?;
        Ok(())
    };
    if web_enabled {
        println!("Preview: http://{}", config.listen);
    }
    #[cfg(feature = "pocketbook")]
    if args.iter().any(|a| a == "--inkview") {
        std::fs::create_dir_all(&config.cache_dir)?;
        let ui_shared = shared.clone();
        let power = config.power.clone();
        let web_worker = tokio::spawn(async move {
            if let Err(error) = server.await {
                eprintln!("Web preview unavailable; calendar remains active: {error}");
            }
        });
        let worker = tokio::spawn(async move {
            if let Err(error) = poll(config, shared, demo).await {
                eprintln!("Calendar background task stopped: {error}");
            }
        });
        let result = pocketbook::run(ui_shared, power);
        worker.abort();
        web_worker.abort();
        return result;
    }
    #[cfg(not(feature = "pocketbook"))]
    anyhow::ensure!(
        !args.iter().any(|a| a == "--inkview"),
        "Build with --features pocketbook and the PocketBook SDK to use --inkview"
    );
    tokio::select! {
        result = poll(config, shared, demo) => result?,
        result = server => result?,
        _ = tokio::signal::ctrl_c() => {}
    }
    Ok(())
}
