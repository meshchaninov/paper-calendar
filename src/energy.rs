use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct PowerConfig {
    pub sleep_enabled: bool,
    pub idle_seconds: u64,
    pub battery_refresh_seconds: u64,
    pub wifi_on_demand: bool,
    pub frontlight_off: bool,
    pub full_refresh_after: u64,
}
impl Default for PowerConfig {
    fn default() -> Self {
        Self {
            sleep_enabled: true,
            idle_seconds: 15,
            battery_refresh_seconds: 3600,
            wifi_on_demand: true,
            frontlight_off: true,
            full_refresh_after: 96,
        }
    }
}
#[derive(Default, Serialize)]
pub struct Metrics {
    pub last_render_ms: u64,
    pub last_paint_ms: u64,
    pub sync_checks: u64,
    pub manual_refreshes: u64,
    pub image_renders: u64,
    pub unchanged_checks: u64,
    pub full_refreshes: u64,
    pub partial_refreshes: u64,
    pub refreshed_pixels: u64,
    pub skipped_frames: u64,
    pub sleep_requests: u64,
    pub observed_suspend_ms: u64,
    pub sleep_refusals: u64,
    pub last_sleep_requested_ms: u64,
    pub last_sleep_wall_ms: u64,
    pub initial_battery_percent: Option<i32>,
    pub session_started_utc: Option<String>,
    pub battery_percent: Option<i32>,
    pub charging: bool,
    pub last_report_utc: Option<String>,
}
#[cfg(any(feature = "pocketbook", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}
#[cfg(any(feature = "pocketbook", test))]
impl Rect {
    pub fn area(&self) -> usize {
        self.width * self.height
    }
}
#[cfg(any(feature = "pocketbook", test))]
pub const WIDTH: usize = 1872;
#[cfg(any(feature = "pocketbook", test))]
pub const HEIGHT: usize = 1404;
// Compare source pixels, not timestamps. Coalesce tile runs vertically so a
// checkbox typically updates one small area. Regions are aligned to 8 pixels.
#[cfg(any(feature = "pocketbook", test))]
pub fn changed_regions(old: &[u8], new: &[u8], width: usize, height: usize) -> Vec<Rect> {
    if new.len() != width * height * 4 {
        return Vec::new();
    }
    if old.len() != new.len() {
        return vec![Rect {
            x: 0,
            y: 0,
            width,
            height,
        }];
    }
    let tile = 32;
    let mut regions: Vec<Rect> = Vec::new();
    for y in (0..height).step_by(tile) {
        let h = tile.min(height - y);
        let mut x = 0;
        while x < width {
            let dirty = |x: usize| {
                let w = tile.min(width - x);
                (y..y + h).any(|row| {
                    let a = (row * width + x) * 4;
                    old[a..a + w * 4] != new[a..a + w * 4]
                })
            };
            if !dirty(x) {
                x += tile;
                continue;
            }
            let start = x;
            x += tile;
            while x < width && dirty(x) {
                x += tile;
            }
            let w = x.min(width) - start;
            if let Some(previous) = regions
                .iter_mut()
                .rev()
                .find(|r| r.x == start && r.width == w && r.y + r.height == y)
            {
                previous.height += h;
            } else {
                regions.push(Rect {
                    x: start,
                    y,
                    width: w,
                    height: h,
                });
            }
        }
    }
    // Bound command count while keeping distant areas separate. A small
    // grid prevents unrelated date and sidebar changes forming a full-page box.
    if regions.len() > 12 {
        let mut grouped = Vec::new();
        for top in (0..height).step_by(352) {
            for left in (0..width).step_by(640) {
                let right = (left + 640).min(width);
                let bottom = (top + 352).min(height);
                let mut bounds: Option<(usize, usize, usize, usize)> = None;
                for r in &regions {
                    let x = r.x.max(left);
                    let y = r.y.max(top);
                    let end_x = (r.x + r.width).min(right);
                    let end_y = (r.y + r.height).min(bottom);
                    if x < end_x && y < end_y {
                        bounds = Some(match bounds {
                            Some((a, b, c, d)) => (a.min(x), b.min(y), c.max(end_x), d.max(end_y)),
                            None => (x, y, end_x, end_y),
                        });
                    }
                }
                if let Some((x, y, right, bottom)) = bounds {
                    grouped.push(Rect {
                        x,
                        y,
                        width: right - x,
                        height: bottom - y,
                    });
                }
            }
        }
        grouped
    } else {
        regions
    }
}
// SDK ibitmap: four u16 fields followed by 8-bit grayscale pixels.
// u16 storage keeps the C header aligned; each buffer owns its pixel data.
#[cfg(any(feature = "pocketbook", test))]
pub fn gray_bitmap(rgba: &[u8], width: usize, rect: Rect) -> Vec<u16> {
    let mut bitmap = vec![0u16; 4 + (rect.width * rect.height).div_ceil(2)];
    bitmap[..4].copy_from_slice(&[rect.width as u16, rect.height as u16, 8, rect.width as u16]);
    for y in 0..rect.height {
        for x in 0..rect.width {
            let pixel = &rgba[((rect.y + y) * width + rect.x + x) * 4..][..4];
            let gray = ((u32::from(pixel[0]) * 77
                + u32::from(pixel[1]) * 150
                + u32::from(pixel[2]) * 29
                + 128)
                >> 8)
                + 255
                - u32::from(pixel[3]);
            let i = y * rect.width + x;
            let shift = (i % 2) * 8;
            bitmap[4 + i / 2] |= (gray.min(255) as u16) << shift;
        }
    }
    bitmap
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bitmap_header_crop_gray_and_alpha_match_sdk_layout() {
        let rgba = vec![
            0, 0, 0, 255, 128, 128, 128, 255, 255, 255, 255, 255, 0, 0, 0, 0, 255, 0, 0, 255, 255,
            255, 255, 255,
        ];
        let bitmap = gray_bitmap(
            &rgba,
            3,
            Rect {
                x: 0,
                y: 0,
                width: 3,
                height: 2,
            },
        );
        assert_eq!(&bitmap[..4], &[3, 2, 8, 3]);
        let data: Vec<_> = bitmap[4..].iter().flat_map(|v| v.to_le_bytes()).collect();
        assert_eq!(data, vec![0, 128, 255, 255, 77, 255]);
        let crop = gray_bitmap(
            &rgba,
            3,
            Rect {
                x: 1,
                y: 0,
                width: 1,
                height: 2,
            },
        );
        assert_eq!(&crop[..4], &[1, 2, 8, 1]);
        assert_eq!(crop[4].to_le_bytes(), [128, 77]);
    }
    #[test]
    fn grouped_regions_cover_every_changed_pixel_without_full_page_box() {
        let old = vec![255; WIDTH * HEIGHT * 4];
        let mut new = old.clone();
        let mut changed = Vec::new();
        for i in 0..60 {
            let x = (i * 103) % WIDTH;
            let y = (i * 167) % HEIGHT;
            new[(y * WIDTH + x) * 4] = 0;
            changed.push((x, y));
        }
        let regions = changed_regions(&old, &new, WIDTH, HEIGHT);
        assert!(regions.len() <= 12);
        for (x, y) in changed {
            assert!(regions
                .iter()
                .any(|r| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height));
        }
        assert!(regions.iter().map(Rect::area).sum::<usize>() < WIDTH * HEIGHT);
    }
    #[test]
    fn unchanged_frame_is_empty() {
        let p = vec![255; 96 * 96 * 4];
        assert!(changed_regions(&p, &p, 96, 96).is_empty());
    }
    #[test]
    fn checkbox_does_not_repaint_the_sidebar() {
        let old = vec![255; WIDTH * HEIGHT * 4];
        let mut new = old.clone();
        for y in 1000..1058 {
            for x in 1490..1548 {
                new[(y * WIDTH + x) * 4] = 0;
            }
        }
        let r = changed_regions(&old, &new, WIDTH, HEIGHT);
        assert!(r.iter().map(Rect::area).sum::<usize>() < 128 * 128);
        assert!(r.iter().all(|r| r.x >= 1472 && r.y >= 992));
    }
    #[test]
    fn distant_changes_are_separate() {
        let old = vec![255; 192 * 128 * 4];
        let mut new = old.clone();
        new[0] = 0;
        new[(127 * 192 + 191) * 4] = 0;
        let r = changed_regions(&old, &new, 192, 128);
        assert_eq!(r.len(), 2);
    }
    #[test]
    fn boundary_pixel_and_initial_frame() {
        let old = vec![255; 33 * 35 * 4];
        let mut new = old.clone();
        *new.last_mut().unwrap() = 0;
        let r = changed_regions(&old, &new, 33, 35);
        assert_eq!(
            r,
            vec![Rect {
                x: 32,
                y: 32,
                width: 1,
                height: 3
            }]
        );
        assert_eq!(
            changed_regions(&[], &new, 33, 35),
            vec![Rect {
                x: 0,
                y: 0,
                width: 33,
                height: 35
            }]
        );
    }
}
