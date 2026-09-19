// Prevents an additional console window on Windows in release; harmless on macOS.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    ableton_music_maker_app_lib::run()
}
