//! The handful of things Lantern remembers between runs, kept in
//! `~/.config/lantern/settings` as `key=value` lines. Not a cache, not an
//! index: just where you were and how the window was set up.

use std::collections::BTreeMap;
use std::path::PathBuf;

use gtk::glib;

pub const LAST_FOLDER: &str = "last-folder";
pub const SIDEBAR: &str = "sidebar";

fn path() -> PathBuf {
    glib::user_config_dir().join("lantern").join("settings")
}

fn read() -> BTreeMap<String, String> {
    std::fs::read_to_string(path())
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

pub fn get(key: &str) -> Option<String> {
    read().remove(key)
}

pub fn set(key: &str, value: &str) {
    let mut settings = read();
    settings.insert(key.to_string(), value.to_string());
    let file = path();
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let text: String = settings.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    let _ = std::fs::write(file, text);
}

pub fn get_bool(key: &str) -> Option<bool> {
    get(key).map(|v| v == "true")
}

pub fn set_bool(key: &str, value: bool) {
    set(key, if value { "true" } else { "false" });
}
