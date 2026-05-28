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

    const PURPLE: [u8; 4] = [124, 58, 237, 255]; // #7C3AED — 与 app 图标紫一致
    const WHITE: [u8; 4] = [255, 255, 255, 255];
    const RED: [u8; 4] = [239, 68, 68, 255];

    // 紫色圆角方背景
    fill_rrect(&mut rgba, TRAY_SIZE, 2.0, 2.0, 30.0, 30.0, 6.5, PURPLE);

    // 白色字母 E:
    //   竖杠 + 顶 / 中 / 底三横，中间一横略短，更接近字体感
    fill_rect(&mut rgba, TRAY_SIZE, 11, 8, 14, 25, WHITE); // vertical bar
    fill_rect(&mut rgba, TRAY_SIZE, 11, 8, 22, 11, WHITE); // top
    fill_rect(&mut rgba, TRAY_SIZE, 11, 14, 20, 17, WHITE); // middle (shorter)
    fill_rect(&mut rgba, TRAY_SIZE, 11, 22, 22, 25, WHITE); // bottom

    if unread {
        // 红色未读角标 + 白色描边，叠在右上角
        fill_circle(&mut rgba, TRAY_SIZE, 24.0, 8.0, 6.2, WHITE);
        fill_circle(&mut rgba, TRAY_SIZE, 24.0, 8.0, 4.8, RED);
    }

    Icon::Rgba {
        rgba,
        width: TRAY_SIZE,
        height: TRAY_SIZE,
    }
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

fn fill_rect(rgba: &mut [u8], width: u32, x0: i32, y0: i32, x1: i32, y1: i32, color: [u8; 4]) {
    let w = width as i32;
    let lo_x = x0.max(0);
    let hi_x = x1.min(w);
    let lo_y = y0.max(0);
    let hi_y = y1.min(w);
    for y in lo_y..hi_y {
        for x in lo_x..hi_x {
            let idx = ((y * w + x) * 4) as usize;
            blend_pixel(rgba, idx, color, 1.0);
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
