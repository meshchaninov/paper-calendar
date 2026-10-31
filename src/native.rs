//! Experimental InkView backend. All SDK calls run on the UI/main thread.
//! ABI reference: koreader/pocketbook-inkview, 517/inkview.h.
//! Cross-compiled for ARM soft-float; loads the device InkView library dynamically.
use crate::{
    energy::{self, PowerConfig},
    Shared,
};
use anyhow::{Context, Result};
use std::{
    collections::VecDeque,
    ffi::{c_char, c_int, c_void},
    sync::{
        atomic::{AtomicBool, AtomicI64, Ordering},
        Mutex, OnceLock,
    },
    time::{Instant, SystemTime},
};

// Load the device's own library at runtime: no SDK library is bundled.
macro_rules! inkview_functions {
    ($(fn $name:ident($($arg:ident: $ty:ty),*) $(-> $ret:ty)?;)*) => {
        #[allow(non_snake_case)]
        struct Api { _library: libloading::Library, $($name: unsafe extern "C" fn($($ty),*) $(-> $ret)?,)* }
        static API: OnceLock<Api> = OnceLock::new();
        impl Api {
            #[allow(non_snake_case)]
            unsafe fn load(library: libloading::Library) -> Result<Self> {
                $(let $name = *library.get::<unsafe extern "C" fn($($ty),*) $(-> $ret)?>(concat!(stringify!($name), "\0").as_bytes())?;)*
                Ok(Self {_library:library,$($name,)*})
            }
        }
        $(#[allow(non_snake_case)]
        unsafe fn $name($($arg:$ty),*) $(-> $ret)? { (API.get().expect("InkView initialized").$name)($($arg),*) })*
    };
}
inkview_functions! {
    fn InkViewMain(handler: extern "C" fn(c_int, c_int, c_int) -> c_int);
    fn SetPanelType(panel_type: c_int);
    fn GetPanelType() -> c_int;
    fn SetClip(x: c_int, y: c_int, width: c_int, height: c_int);
    fn ClearScreen();
    fn SetOrientation(orientation: c_int);
    fn GetOrientation() -> c_int;
    fn ScreenWidth() -> c_int;
    fn ScreenHeight() -> c_int;
    fn DrawBitmap(x: c_int, y: c_int, bitmap: *const c_void);
    fn FullUpdate();
    fn PartialUpdate(x:c_int,y:c_int,width:c_int,height:c_int);
    fn FillArea(x:c_int,y:c_int,width:c_int,height:c_int,color:c_int);
    fn SetHardTimer(name: *const c_char, callback: extern "C" fn(), ms: c_int);
    fn CloseApp();
}
// Power APIs vary across firmware. Missing optional symbols must never
// prevent the already-working calendar from opening.
macro_rules! optional_power {
    ($(fn $name:ident($($arg:ident:$ty:ty),*) -> $ret:ty = $fallback:expr;)*) => {$ (
        #[allow(non_snake_case)]
        unsafe fn $name($($arg:$ty),*) -> $ret {
            static SYMBOL:OnceLock<Option<unsafe extern "C" fn($($ty),*)->$ret>>=OnceLock::new();
            let symbol=SYMBOL.get_or_init(||API.get().and_then(|api|api._library.get::<unsafe extern "C" fn($($ty),*)->$ret>(concat!(stringify!($name),"\0").as_bytes()).ok().map(|s|*s)));
            match symbol {Some(function)=>function($($arg),*),None=>$fallback}
        }
    )*};
}
optional_power! {
 fn GoSleep(ms:c_int,deep:c_int)->c_int = -1;
 fn iv_sleepmode(enabled:c_int)->() = ();
 fn GetSleepmode()->c_int = 0;
 fn IsTaskActive()->c_int = 1;
 fn get_keylock()->c_int = 0;
 fn IsCharging()->c_int = 0;
 fn GetBatteryPower()->c_int = -1;
 fn WiFiPower(state:c_int)->c_int = -1;
 fn NetConnectSilent(name:*const c_char)->c_int = -1;
 fn NetDisconnect()->c_int = -1;
 fn GetFrontlightState()->c_int = -1;
 fn SetFrontlightStateEx(state:c_int,temporary:c_int)->() = ();
 fn SetAutoPowerOff(state:c_int)->() = ();
}
enum Input {
    Tap(i32, i32),
    Month(i32),
}
struct Ui {
    shared: Shared,
    power: PowerConfig,
    taps: Mutex<VecDeque<Input>>,
    shown: Mutex<String>,
    previous: Mutex<Vec<u8>>,
    partial_count: Mutex<u64>,
    partial_area: Mutex<usize>,
    visible: Mutex<bool>,
    pending_restore: AtomicBool,
    grace_until: AtomicI64,
    cooldown_until: AtomicI64,
    previous_light: Mutex<Option<i32>>,
    previous_sleep: Mutex<Option<i32>>,
}
static UI: OnceLock<Ui> = OnceLock::new();
fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
fn paint(force: bool) {
    let Some(ui) = UI.get() else {
        return;
    };
    if force {
        ui.pending_restore.store(true, Ordering::Relaxed);
    }
    let force = ui.pending_restore.load(Ordering::Relaxed);
    if !ui.visible.lock().map(|v| *v).unwrap_or(false) {
        return;
    }
    let Ok(mut screen) = ui.shared.try_write() else {
        return;
    };
    let Ok(mut shown) = ui.shown.lock() else {
        return;
    };
    if screen.rgba.is_empty() {
        return;
    }
    if !force && *shown == screen.etag {
        screen.metrics.skipped_frames += 1;
        return;
    }
    let Ok(mut previous) = ui.previous.lock() else {
        return;
    };
    let paint_started = Instant::now();
    let regions = energy::changed_regions(&previous, &screen.rgba, energy::WIDTH, energy::HEIGHT);
    if !force && regions.is_empty() {
        *shown = screen.etag.clone();
        screen.metrics.skipped_frames += 1;
        return;
    }
    let Ok(mut count) = ui.partial_count.lock() else {
        return;
    };
    let Ok(mut area) = ui.partial_area.lock() else {
        return;
    };
    let full = force
        || previous.is_empty()
        || (ui.power.full_refresh_after > 0 && *count >= ui.power.full_refresh_after)
        || *area >= energy::WIDTH * energy::HEIGHT * 3;
    unsafe {
        SetPanelType(0);
        if ScreenWidth() != 1872 || ScreenHeight() != 1404 {
            return;
        }
        if full {
            SetClip(0, 0, 1872, 1404);
            ClearScreen();
            let bitmap = energy::gray_bitmap(
                &screen.rgba,
                energy::WIDTH,
                energy::Rect {
                    x: 0,
                    y: 0,
                    width: energy::WIDTH,
                    height: energy::HEIGHT,
                },
            );
            DrawBitmap(0, 0, bitmap.as_ptr().cast());
            FullUpdate();
            screen.metrics.full_refreshes += 1;
            screen.metrics.refreshed_pixels += (energy::WIDTH * energy::HEIGHT) as u64;
            *count = 0;
            *area = 0;
        } else {
            for r in &regions {
                SetClip(r.x as i32, r.y as i32, r.width as i32, r.height as i32);
                FillArea(
                    r.x as i32,
                    r.y as i32,
                    r.width as i32,
                    r.height as i32,
                    0xffffff,
                );
                let bitmap = energy::gray_bitmap(&screen.rgba, energy::WIDTH, *r);
                DrawBitmap(r.x as i32, r.y as i32, bitmap.as_ptr().cast());
                PartialUpdate(r.x as i32, r.y as i32, r.width as i32, r.height as i32);
                screen.metrics.partial_refreshes += 1;
                screen.metrics.refreshed_pixels += r.area() as u64;
                *area += r.area();
            }
            *count += 1;
        }
        SetClip(0, 0, 1872, 1404);
    }
    screen.metrics.last_paint_ms = paint_started.elapsed().as_millis() as u64;
    previous.clone_from(&screen.rgba);
    *shown = screen.etag.clone();
    ui.pending_restore.store(false, Ordering::Relaxed);
}
fn schedule(ms: i32) {
    unsafe {
        SetHardTimer(c"paper-calendar".as_ptr(), tick, ms.max(100));
    }
}
fn user_active() {
    if let Some(ui) = UI.get() {
        ui.grace_until
            .store(now() + ui.power.idle_seconds as i64, Ordering::Relaxed);
        unsafe {
            iv_sleepmode(0);
        }
    }
}
fn process_radio(ui: &Ui) {
    let request = if let Ok(mut s) = ui.shared.try_write() {
        let r = s.network_request;
        s.network_request = 0;
        r
    } else {
        0
    };
    if request == 0 {
        return;
    }
    let allowed = request < 0 || ui.visible.lock().map(|v| *v).unwrap_or(false);
    let success = unsafe {
        if !allowed {
            false
        } else if request > 0 {
            WiFiPower(1);
            NetConnectSilent(std::ptr::null()) == 0
        } else {
            NetDisconnect();
            WiFiPower(0);
            true
        }
    };
    if let Ok(mut s) = ui.shared.try_write() {
        s.network_result = success;
        s.network_done.notify_one();
    }
}
fn process_taps(shared: &Shared, taps: &Mutex<VecDeque<Input>>) {
    let Ok(mut screen) = shared.try_write() else {
        return;
    };
    if let Ok(mut taps) = taps.lock() {
        while let Some(input) = taps.pop_front() {
            let result = match input {
                Input::Tap(x, y) => crate::todo::tap(&mut screen, x, y),
                Input::Month(delta) => crate::todo::change_month(&mut screen, delta),
            };
            if let Err(error) = result {
                eprintln!("Could not handle tap: {error}");
            }
        }
    }
}
extern "C" fn tick() {
    let Some(ui) = UI.get() else {
        return;
    };
    process_radio(ui);
    process_taps(&ui.shared, &ui.taps);
    if let Ok(mut screen) = ui.shared.try_write() {
        if let Err(error) = crate::todo::return_home_if_idle(&mut screen, now()) {
            eprintln!("Could not restore today: {error}");
        }
    }
    paint(false);
    let charging = unsafe { IsCharging() > 0 };
    let battery = unsafe { GetBatteryPower() };
    let Some((busy, next, ready, first_sleep)) = ui.shared.try_write().ok().map(|mut s| {
        if s.metrics.charging != charging {
            s.metrics.charging = charging;
            s.wake.notify_one();
        }
        if s.metrics.initial_battery_percent.is_none() && (0..=100).contains(&battery) {
            s.metrics.initial_battery_percent = Some(battery);
            s.metrics.session_started_utc = Some(chrono::Utc::now().to_rfc3339());
        }
        s.metrics.battery_percent = if (0..=100).contains(&battery) {
            Some(battery)
        } else {
            None
        };
        (
            s.busy || s.network_request != 0,
            s.next_check,
            !s.rgba.is_empty(),
            s.metrics.sleep_requests == 0,
        )
    }) else {
        schedule(250);
        return;
    };
    if !ui.visible.lock().map(|v| *v).unwrap_or(false) {
        unsafe {
            iv_sleepmode(1);
        }
        schedule(60000);
        return;
    }
    if busy || !ready {
        unsafe {
            iv_sleepmode(0);
        }
        schedule(250);
        return;
    }
    let time = now();
    if next <= time {
        if let Ok(s) = ui.shared.try_read() {
            s.wake.notify_one();
        }
        schedule(250);
        return;
    }
    let grace = ui.grace_until.load(Ordering::Relaxed);
    if time < grace {
        schedule(((grace - time).min(next - time) * 1000).clamp(100, 15000) as i32);
        return;
    }
    if !ui.power.sleep_enabled || charging || ui.cooldown_until.load(Ordering::Relaxed) > time {
        schedule(((next - time) * 1000).clamp(100, 5000) as i32);
        return;
    }
    if unsafe { IsTaskActive() == 0 || get_keylock() != 0 } {
        schedule(60000);
        return;
    }
    let limit = if first_sleep {
        60000
    } else {
        ui.power
            .battery_refresh_seconds
            .saturating_mul(1000)
            .clamp(1000, 86400000) as i64
    };
    let ms = ((next - time) * 1000).clamp(1000, limit) as i32;
    let wall = SystemTime::now();
    let monotonic = Instant::now();
    unsafe {
        iv_sleepmode(1);
        GoSleep(ms, 0);
        iv_sleepmode(0);
    }
    let elapsed = wall.elapsed().unwrap_or_default().as_millis() as u64;
    let suspended = elapsed.saturating_sub(monotonic.elapsed().as_millis() as u64);
    if let Ok(mut s) = ui.shared.try_write() {
        s.metrics.sleep_requests += 1;
        s.metrics.last_sleep_requested_ms = ms as u64;
        s.metrics.last_sleep_wall_ms = elapsed;
        if suspended >= 500 {
            s.metrics.observed_suspend_ms += suspended;
        }
        if elapsed < 100 {
            s.metrics.sleep_refusals += 1;
            ui.cooldown_until.store(now() + 5, Ordering::Relaxed);
        }
        s.wake.notify_one();
        let _ = crate::save_energy_report(&mut s);
    }
    // Give queued touch/power events and the worker a chance to run before the
    // next sleep. Sleep itself does not invalidate the e-ink image.
    ui.grace_until.store(now() + 1, Ordering::Relaxed);
    schedule(100);
}
extern "C" fn event(kind: c_int, x: c_int, y: c_int) -> c_int {
    match kind {
        21 => {
            // EVT_INIT
            unsafe {
                if let Some(ui) = UI.get() {
                    if let Ok(mut previous) = ui.previous_sleep.lock() {
                        *previous = Some(GetSleepmode());
                    }
                    iv_sleepmode(0);
                    SetAutoPowerOff(0);
                    if ui.power.frontlight_off {
                        let light = GetFrontlightState();
                        if (0..=100).contains(&light) {
                            if let Ok(mut previous) = ui.previous_light.lock() {
                                *previous = Some(light);
                            }
                            SetFrontlightStateEx(0, 1);
                        }
                    }
                    if ui.power.wifi_on_demand {
                        WiFiPower(0);
                    }
                    user_active();
                }
                SetPanelType(0);
                SetOrientation(1);
                SetClip(0, 0, ScreenWidth(), ScreenHeight());
                ClearScreen();
                eprintln!(
                    "InkView fullscreen initialized: {}x{}, panel={}",
                    ScreenWidth(),
                    ScreenHeight(),
                    GetPanelType()
                );
            }
            tick();
        }
        23 => {
            // EVT_SHOW
            let restore = UI
                .get()
                .and_then(|ui| {
                    ui.visible.lock().ok().map(|mut visible| {
                        let restore = !*visible;
                        *visible = true;
                        restore
                    })
                })
                .unwrap_or(false);
            // A wake leaves the e-ink image intact. Restore only if another app
            // actually hid our surface; repeated SHOW notifications are free.
            paint(restore);
            user_active();
            schedule(100);
        }
        24 | 22 => {
            // EVT_HIDE / EVT_EXIT
            if let Some(ui) = UI.get() {
                if let Ok(mut v) = ui.visible.lock() {
                    *v = false;
                }
                if let Ok(mut screen) = ui.shared.try_write() {
                    let _ = crate::save_energy_report(&mut screen);
                }
            }
        }
        25 => {
            // InkView KEY_PREV/NEXT (+ alternate page keys). Handle only
            // key-down, so key-up/repeat cannot advance the month twice.
            let delta = match x {
                0x18 | 0x1c | 0x13 => -1,
                0x19 | 0x1d | 0x14 => 1,
                _ => return 0,
            };
            user_active();
            if let Some(ui) = UI.get() {
                if ui.visible.lock().map(|v| *v).unwrap_or(false) {
                    if let Ok(mut taps) = ui.taps.lock() {
                        taps.push_back(Input::Month(delta));
                    }
                    process_taps(&ui.shared, &ui.taps);
                    paint(false);
                    schedule(100);
                }
            }
            return 1;
        }
        30 | 31 => {
            // Touch-down/movement counts as activity even before pointer-up.
            user_active();
            if let Some(ui) = UI.get() {
                if let Ok(mut screen) = ui.shared.try_write() {
                    crate::todo::activity(&mut screen);
                }
            }
            schedule(100);
        }
        29 => {
            user_active();
            // EVT_POINTERUP: same large row hit areas as browser preview.
            if let Some(ui) = UI.get() {
                if ui.visible.lock().map(|v| *v).unwrap_or(false) {
                    if let Ok(mut taps) = ui.taps.lock() {
                        taps.push_back(Input::Tap(x, y));
                    }
                    process_taps(&ui.shared, &ui.taps);
                    paint(false);
                    schedule(100);
                }
            }
        }
        34 => {
            let returned = UI
                .get()
                .and_then(|ui| {
                    ui.shared
                        .try_write()
                        .ok()
                        .map(|mut screen| crate::todo::back(&mut screen).unwrap_or(false))
                })
                .unwrap_or(false);
            if returned {
                paint(false);
            } else {
                unsafe {
                    CloseApp();
                }
            }
        } // EVT_POINTERLONG
        _ => {}
    }
    0
}
pub fn run(shared: Shared, power: PowerConfig) -> Result<()> {
    let library = unsafe {
        libloading::Library::new("libinkview.so")
            .or_else(|_| libloading::Library::new("/ebrmain/lib/libinkview.so"))
    }
    .context("Load the PocketBook InkView runtime")?;
    let api = unsafe { Api::load(library) }?;
    API.set(api)
        .map_err(|_| anyhow::anyhow!("InkView runtime already loaded"))?;
    UI.set(Ui {
        shared,
        power,
        taps: Mutex::new(VecDeque::new()),
        shown: Mutex::new(String::new()),
        previous: Mutex::new(Vec::new()),
        partial_count: Mutex::new(0),
        partial_area: Mutex::new(0),
        visible: Mutex::new(true),
        pending_restore: AtomicBool::new(false),
        grace_until: AtomicI64::new(0),
        cooldown_until: AtomicI64::new(0),
        previous_light: Mutex::new(None),
        previous_sleep: Mutex::new(None),
    })
    .map_err(|_| anyhow::anyhow!("InkView UI already started"))?;
    unsafe {
        let previous = GetOrientation();
        InkViewMain(event);
        SetOrientation(previous);
        if let Some(ui) = UI.get() {
            if let Ok(light) = ui.previous_light.lock() {
                if let Some(value) = *light {
                    SetFrontlightStateEx(value, 1);
                }
            }
            if let Ok(sleep) = ui.previous_sleep.lock() {
                if let Some(value) = *sleep {
                    iv_sleepmode(value);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn busy_calendar_keeps_taps_until_state_is_available() {
        let shared: Shared = Default::default();
        let taps = Mutex::new(VecDeque::from([Input::Tap(300, 200), Input::Tap(1140, 50)]));
        let guard = shared.try_write().unwrap();
        process_taps(&shared, &taps);
        assert_eq!(taps.lock().unwrap().len(), 2);
        drop(guard);
        process_taps(&shared, &taps);
        assert!(taps.lock().unwrap().is_empty());
        assert!(shared.try_read().unwrap().idle_until.is_some());
    }
}
