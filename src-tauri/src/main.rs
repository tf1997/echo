#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    echo::updater::relaunch_latest_portable_if_needed();
    echo::run();
}
