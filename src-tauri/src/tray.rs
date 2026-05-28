use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tauri::{
    AppHandle, CustomMenuItem, Icon, Manager, SystemTray, SystemTrayEvent, SystemTrayMenu,
    SystemTrayMenuItem, UserAttentionType,
};

use crate::updater;

const TRAY_SHOW: &str = "tray_show";
const TRAY_QUIT: &str = "tray_quit";
const TRAY_SIZE: u32 = 32;

static SHOULD_FLASH: AtomicBool = AtomicBool::new(false);
static FLASH_TASK_RUNNING: AtomicBool = AtomicBool::new(false);

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

pub fn set_unread_attention(app: &AppHandle, active: bool) -> tauri::Result<()> {
    SHOULD_FLASH.store(active, Ordering::SeqCst);
    set_taskbar_attention(app, active);

    if active {
        app.tray_handle().set_tooltip("Echo - 有未读消息")?;
        if FLASH_TASK_RUNNING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let app_handle = app.clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    let mut show_badge = true;
                    while SHOULD_FLASH.load(Ordering::SeqCst) {
                        let tray = app_handle.tray_handle();
                        let _ = tray.set_icon(tray_icon(show_badge));
                        let _ = tray.set_tooltip(if show_badge {
                            "Echo - 有未读消息"
                        } else {
                            "Echo"
                        });
                        show_badge = !show_badge;
                        tokio::time::sleep(Duration::from_millis(650)).await;
                    }

                    let tray = app_handle.tray_handle();
                    let _ = tray.set_icon(tray_icon(false));
                    let _ = tray.set_tooltip("Echo");
                    FLASH_TASK_RUNNING.store(false, Ordering::SeqCst);

                    if !SHOULD_FLASH.load(Ordering::SeqCst)
                        || FLASH_TASK_RUNNING
                            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                            .is_err()
                    {
                        break;
                    }
                }
            });
        }
    } else {
        app.tray_handle().set_icon(tray_icon(false))?;
        app.tray_handle().set_tooltip("Echo")?;
    }

    Ok(())
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
    let mut rgba = vec![0; size * size * 4];

    draw_circle(&mut rgba, TRAY_SIZE, 16.0, 16.0, 13.5, [7, 193, 96, 255]);
    draw_circle(&mut rgba, TRAY_SIZE, 16.0, 16.0, 7.0, [255, 255, 255, 255]);
    draw_circle(&mut rgba, TRAY_SIZE, 16.0, 16.0, 4.2, [7, 193, 96, 255]);

    if unread {
        draw_circle(&mut rgba, TRAY_SIZE, 23.0, 9.0, 7.2, [255, 255, 255, 255]);
        draw_circle(&mut rgba, TRAY_SIZE, 23.0, 9.0, 5.4, [239, 68, 68, 255]);
    }

    Icon::Rgba {
        rgba,
        width: TRAY_SIZE,
        height: TRAY_SIZE,
    }
}

fn draw_circle(rgba: &mut [u8], width: u32, cx: f32, cy: f32, radius: f32, color: [u8; 4]) {
    let width_usize = width as usize;
    let radius_sq = radius * radius;
    for y in 0..width_usize {
        for x in 0..width_usize {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            if dx * dx + dy * dy <= radius_sq {
                let idx = (y * width_usize + x) * 4;
                rgba[idx..idx + 4].copy_from_slice(&color);
            }
        }
    }
}
