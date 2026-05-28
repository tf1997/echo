use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, CustomMenuItem, Icon, Manager, SystemTray, SystemTrayEvent, SystemTrayMenu,
    SystemTrayMenuItem, UserAttentionType,
};

use crate::updater;

const TRAY_SHOW: &str = "tray_show";
const TRAY_QUIT: &str = "tray_quit";
const TRAY_SIZE: u32 = 32;
const TOOLTIP_MAX_ITEMS: usize = 5;
const TOOLTIP_MAX_CHARS: usize = 120;
const EVENT_OPEN_CONVERSATION: &str = "tray-open-conversation";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrayUnreadItem {
    pub kind: String,
    pub id: String,
    pub name: String,
    pub count: u32,
    #[serde(default)]
    pub last_ts: i64,
}

static SHOULD_FLASH: AtomicBool = AtomicBool::new(false);
static FLASH_TASK_RUNNING: AtomicBool = AtomicBool::new(false);
static WINDOW_FOCUSED: AtomicBool = AtomicBool::new(true);

fn snapshot_store() -> &'static Mutex<Vec<TrayUnreadItem>> {
    static SNAPSHOT: OnceLock<Mutex<Vec<TrayUnreadItem>>> = OnceLock::new();
    SNAPSHOT.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn system_tray(check_update_id: &str) -> SystemTray {
    let menu = SystemTrayMenu::new()
        .add_item(CustomMenuItem::new(TRAY_SHOW, "显示 Echo"))
        .add_item(CustomMenuItem::new(check_update_id, "检查更新"))
        .add_native_item(SystemTrayMenuItem::Separator)
        .add_item(CustomMenuItem::new(TRAY_QUIT, "退出"));

    SystemTray::new()
        .with_icon(tray_icon(false))
        .with_tooltip("Echo")
        .with_menu(menu)
}

pub fn handle_tray_event(app: &AppHandle, event: SystemTrayEvent, check_update_id: &str) {
    match event {
        SystemTrayEvent::LeftClick { .. } | SystemTrayEvent::DoubleClick { .. } => {
            show_main_window(app);
            emit_top_unread(app);
        }
        SystemTrayEvent::MenuItemClick { id, .. } if id == TRAY_SHOW => {
            show_main_window(app);
        }
        SystemTrayEvent::MenuItemClick { id, .. } if id == check_update_id => {
            updater::spawn_manual_update_check(app.clone());
        }
        SystemTrayEvent::MenuItemClick { id, .. } if id == TRAY_QUIT => {
            app.exit(0);
        }
        _ => {}
    }
}

pub fn update_unread(app: &AppHandle, items: Vec<TrayUnreadItem>) -> tauri::Result<()> {
    let total: u32 = items.iter().map(|item| item.count).sum();
    let tooltip = build_tooltip(&items, total);

    {
        let mut store = snapshot_store().lock().unwrap();
        *store = items;
    }

    let has_unread = total > 0;
    let should_flash = has_unread && !WINDOW_FOCUSED.load(Ordering::SeqCst);
    SHOULD_FLASH.store(should_flash, Ordering::SeqCst);
    set_taskbar_attention(app, should_flash);

    app.tray_handle().set_tooltip(&tooltip)?;

    if should_flash {
        ensure_flash_task(app);
    } else {
        app.tray_handle().set_icon(tray_icon(false))?;
    }
    Ok(())
}

pub fn note_window_focused(app: &AppHandle, focused: bool) {
    WINDOW_FOCUSED.store(focused, Ordering::SeqCst);
    if focused {
        if SHOULD_FLASH.load(Ordering::SeqCst) {
            SHOULD_FLASH.store(false, Ordering::SeqCst);
            let _ = app.tray_handle().set_icon(tray_icon(false));
            set_taskbar_attention(app, false);
        }
    } else {
        let has_unread = {
            let store = snapshot_store().lock().unwrap();
            store.iter().any(|item| item.count > 0)
        };
        if has_unread && !SHOULD_FLASH.load(Ordering::SeqCst) {
            SHOULD_FLASH.store(true, Ordering::SeqCst);
            set_taskbar_attention(app, true);
            ensure_flash_task(app);
        }
    }
}

fn ensure_flash_task(app: &AppHandle) {
    if FLASH_TASK_RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut show_badge = true;
        while SHOULD_FLASH.load(Ordering::SeqCst) {
            let tray = app_handle.tray_handle();
            let _ = tray.set_icon(tray_icon(show_badge));
            show_badge = !show_badge;
            tokio::time::sleep(Duration::from_millis(650)).await;
        }
        let _ = app_handle.tray_handle().set_icon(tray_icon(false));
        FLASH_TASK_RUNNING.store(false, Ordering::SeqCst);
    });
}

fn build_tooltip(items: &[TrayUnreadItem], total: u32) -> String {
    if total == 0 || items.is_empty() {
        return "Echo".to_string();
    }
    let mut sorted: Vec<&TrayUnreadItem> = items.iter().collect();
    sorted.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));

    let mut tooltip = format!("Echo · {} 条未读", total);
    let shown = sorted.len().min(TOOLTIP_MAX_ITEMS);
    for item in sorted.iter().take(shown) {
        let line = format!("\n• {} ({})", item.name, item.count);
        if tooltip.chars().count() + line.chars().count() > TOOLTIP_MAX_CHARS {
            break;
        }
        tooltip.push_str(&line);
    }
    let remaining = sorted.len().saturating_sub(shown);
    if remaining > 0 {
        let suffix = format!("\n…还有 {} 个", remaining);
        if tooltip.chars().count() + suffix.chars().count() <= TOOLTIP_MAX_CHARS {
            tooltip.push_str(&suffix);
        }
    }
    tooltip
}

fn emit_top_unread(app: &AppHandle) {
    let top = {
        let store = snapshot_store().lock().unwrap();
        let mut sorted: Vec<TrayUnreadItem> = store.clone();
        sorted.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));
        sorted.into_iter().next()
    };
    if let Some(item) = top {
        let _ = app.emit_all(EVENT_OPEN_CONVERSATION, item);
    }
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn set_taskbar_attention(app: &AppHandle, active: bool) {
    if let Some(window) = app.get_window("main") {
        let request = if active {
            Some(UserAttentionType::Critical)
        } else {
            None
        };
        let _ = window.request_user_attention(request);
    }
}

fn tray_icon(unread: bool) -> Icon {
    let size = TRAY_SIZE as usize;
    let mut rgba = vec![0u8; size * size * 4];

    const TOP: [u8; 4] = [155, 124, 255, 255];
    const MID: [u8; 4] = [108, 59, 255, 255];
    const BOTTOM: [u8; 4] = [9, 182, 242, 255];
    const MARK: [u8; 4] = [255, 255, 255, 246];
    const MARK_SHADOW: [u8; 4] = [25, 10, 80, 78];
    const RED: [u8; 4] = [255, 59, 48, 255];

    fill_rrect_gradient(
        &mut rgba, TRAY_SIZE, 2.0, 2.0, 30.0, 30.0, 7.2, TOP, MID, BOTTOM,
    );
    fill_circle(&mut rgba, TRAY_SIZE, 8.2, 2.2, 13.8, [255, 255, 255, 48]);
    fill_circle(&mut rgba, TRAY_SIZE, 24.0, 25.0, 11.8, [126, 235, 255, 62]);
    stroke_rrect(
        &mut rgba,
        TRAY_SIZE,
        2.4,
        2.4,
        29.6,
        29.6,
        6.9,
        1.1,
        [255, 255, 255, 70],
    );

    draw_tray_mark(&mut rgba, 0.0, 0.8, MARK_SHADOW);
    draw_tray_mark(&mut rgba, 0.0, 0.0, MARK);

    if unread {
        fill_circle(&mut rgba, TRAY_SIZE, 24.2, 7.8, 6.3, MARK);
        fill_circle(&mut rgba, TRAY_SIZE, 24.2, 7.8, 4.8, RED);
        fill_circle(&mut rgba, TRAY_SIZE, 22.6, 6.0, 1.35, [255, 255, 255, 122]);
    }

    Icon::Rgba {
        rgba,
        width: TRAY_SIZE,
        height: TRAY_SIZE,
    }
}

fn draw_tray_mark(rgba: &mut [u8], dx: f32, dy: f32, color: [u8; 4]) {
    fill_rrect(
        rgba,
        TRAY_SIZE,
        9.2 + dx,
        7.5 + dy,
        13.2 + dx,
        24.9 + dy,
        2.0,
        color,
    );
    fill_rrect(
        rgba,
        TRAY_SIZE,
        9.2 + dx,
        7.5 + dy,
        23.2 + dx,
        11.6 + dy,
        2.05,
        color,
    );
    fill_rrect(
        rgba,
        TRAY_SIZE,
        9.2 + dx,
        14.1 + dy,
        20.4 + dx,
        18.2 + dy,
        2.05,
        color,
    );
    fill_rrect(
        rgba,
        TRAY_SIZE,
        9.2 + dx,
        20.8 + dy,
        23.2 + dx,
        24.9 + dy,
        2.05,
        color,
    );
}

fn fill_rrect_gradient(
    rgba: &mut [u8],
    width: u32,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    r: f32,
    top: [u8; 4],
    mid: [u8; 4],
    bottom: [u8; 4],
) {
    let w = width as usize;
    let cx = (x0 + x1) * 0.5;
    let cy = (y0 + y1) * 0.5;
    let bx = (x1 - x0) * 0.5;
    let by = (y1 - y0) * 0.5;
    for y in 0..w {
        for x in 0..w {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let qx = (px - cx).abs() - bx + r;
            let qy = (py - cy).abs() - by + r;
            let outside = qx.max(0.0).hypot(qy.max(0.0));
            let inside = qx.max(qy).min(0.0);
            let sdf = outside + inside - r;
            let coverage = (0.5 - sdf).clamp(0.0, 1.0);
            if coverage > 0.0 {
                let tx = ((px - x0) / (x1 - x0)).clamp(0.0, 1.0);
                let ty = ((py - y0) / (y1 - y0)).clamp(0.0, 1.0);
                let t = (tx * 0.28 + ty * 0.72).clamp(0.0, 1.0);
                let color = if t < 0.52 {
                    lerp_color(top, mid, t / 0.52)
                } else {
                    lerp_color(mid, bottom, (t - 0.52) / 0.48)
                };
                blend_pixel(rgba, (y * w + x) * 4, color, coverage);
            }
        }
    }
}

fn stroke_rrect(
    rgba: &mut [u8],
    width: u32,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    r: f32,
    thickness: f32,
    color: [u8; 4],
) {
    let w = width as usize;
    let cx = (x0 + x1) * 0.5;
    let cy = (y0 + y1) * 0.5;
    let bx = (x1 - x0) * 0.5;
    let by = (y1 - y0) * 0.5;
    let half = thickness * 0.5;
    for y in 0..w {
        for x in 0..w {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let qx = (px - cx).abs() - bx + r;
            let qy = (py - cy).abs() - by + r;
            let outside = qx.max(0.0).hypot(qy.max(0.0));
            let inside = qx.max(qy).min(0.0);
            let sdf = outside + inside - r;
            let coverage = (0.5 - (sdf.abs() - half)).clamp(0.0, 1.0);
            if coverage > 0.0 {
                blend_pixel(rgba, (y * w + x) * 4, color, coverage);
            }
        }
    }
}

fn lerp_color(a: [u8; 4], b: [u8; 4], t: f32) -> [u8; 4] {
    let t = t.clamp(0.0, 1.0);
    [
        (a[0] as f32 + (b[0] as f32 - a[0] as f32) * t).round() as u8,
        (a[1] as f32 + (b[1] as f32 - a[1] as f32) * t).round() as u8,
        (a[2] as f32 + (b[2] as f32 - a[2] as f32) * t).round() as u8,
        (a[3] as f32 + (b[3] as f32 - a[3] as f32) * t).round() as u8,
    ]
}

fn fill_rrect(
    rgba: &mut [u8],
    width: u32,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    r: f32,
    color: [u8; 4],
) {
    let w = width as usize;
    let cx = (x0 + x1) * 0.5;
    let cy = (y0 + y1) * 0.5;
    let bx = (x1 - x0) * 0.5;
    let by = (y1 - y0) * 0.5;
    for y in 0..w {
        for x in 0..w {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let qx = (px - cx).abs() - bx + r;
            let qy = (py - cy).abs() - by + r;
            let outside = qx.max(0.0).hypot(qy.max(0.0));
            let inside = qx.max(qy).min(0.0);
            let sdf = outside + inside - r;
            let coverage = (0.5 - sdf).clamp(0.0, 1.0);
            if coverage > 0.0 {
                blend_pixel(rgba, (y * w + x) * 4, color, coverage);
            }
        }
    }
}

fn fill_circle(rgba: &mut [u8], width: u32, cx: f32, cy: f32, r: f32, color: [u8; 4]) {
    let w = width as usize;
    for y in 0..w {
        for x in 0..w {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let dx = px - cx;
            let dy = py - cy;
            let d = (dx * dx + dy * dy).sqrt();
            let coverage = (0.5 - (d - r)).clamp(0.0, 1.0);
            if coverage > 0.0 {
                blend_pixel(rgba, (y * w + x) * 4, color, coverage);
            }
        }
    }
}

fn blend_pixel(rgba: &mut [u8], idx: usize, color: [u8; 4], coverage: f32) {
    let src_a = coverage * (color[3] as f32 / 255.0);
    if src_a <= 0.0 {
        return;
    }
    let dst_a = rgba[idx + 3] as f32 / 255.0;
    let out_a = src_a + dst_a * (1.0 - src_a);
    if out_a < 1e-6 {
        return;
    }
    for c in 0..3 {
        let src = color[c] as f32;
        let dst = rgba[idx + c] as f32;
        let out = (src * src_a + dst * dst_a * (1.0 - src_a)) / out_a;
        rgba[idx + c] = out.round().clamp(0.0, 255.0) as u8;
    }
    rgba[idx + 3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
}
