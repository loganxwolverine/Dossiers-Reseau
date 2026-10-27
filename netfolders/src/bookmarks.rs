//! Raccourcis dans la barre latérale de Fichiers (~/.config/gtk-3.0/bookmarks).

use std::path::{Path, PathBuf};

use gtk::{gio, glib};
use gtk::prelude::FileExt;

fn file() -> PathBuf {
    glib::user_config_dir().join("gtk-3.0").join("bookmarks")
}

fn uri(dir: &Path) -> String {
    gio::File::for_path(dir).uri().to_string()
}

pub fn contains(dir: &Path) -> bool {
    let uri = uri(dir);
    std::fs::read_to_string(file())
        .unwrap_or_default()
        .lines()
        .any(|l| l.split(' ').next() == Some(uri.as_str()))
}

pub fn add(dir: &Path, label: &str) {
    let path = file();
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    let uri = uri(dir);
    if content.lines().any(|l| l.split(' ').next() == Some(uri.as_str())) {
        return;
    }
    let mut content = content;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(&format!("{uri} {label}\n"));
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, content);
}

pub fn remove(dir: &Path) {
    let path = file();
    let Ok(content) = std::fs::read_to_string(&path) else { return };
    let uri = uri(dir);
    let kept: Vec<&str> = content
        .lines()
        .filter(|l| l.split(' ').next() != Some(uri.as_str()))
        .collect();
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    let _ = std::fs::write(&path, out);
}
