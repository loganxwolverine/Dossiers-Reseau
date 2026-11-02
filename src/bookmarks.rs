//! Raccourcis dans la barre latérale du gestionnaire de fichiers :
//! - Fichiers (GNOME) et sélecteurs de fichiers GTK : ~/.config/gtk-3.0/bookmarks
//! - Dolphin (KDE, bureau par défaut de Bazzite) : ~/.local/share/user-places.xbel

use std::path::{Path, PathBuf};

use gtk::prelude::FileExt;
use gtk::{gio, glib};

fn gtk_file() -> PathBuf {
    glib::user_config_dir().join("gtk-3.0").join("bookmarks")
}

fn kde_file() -> PathBuf {
    glib::user_data_dir().join("user-places.xbel")
}

fn uri(dir: &Path) -> String {
    gio::File::for_path(dir).uri().to_string()
}

fn kde_marker(uri: &str) -> String {
    format!("<bookmark href=\"{}\"", glib::markup_escape_text(uri))
}

pub fn contains(dir: &Path) -> bool {
    let uri = uri(dir);
    let gtk = std::fs::read_to_string(gtk_file())
        .unwrap_or_default()
        .lines()
        .any(|l| l.split(' ').next() == Some(uri.as_str()));
    let kde = std::fs::read_to_string(kde_file())
        .unwrap_or_default()
        .contains(&kde_marker(&uri));
    gtk || kde
}

pub fn add(dir: &Path, label: &str) {
    let uri = uri(dir);

    let path = gtk_file();
    let mut content = std::fs::read_to_string(&path).unwrap_or_default();
    if !content.lines().any(|l| l.split(' ').next() == Some(uri.as_str())) {
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(&format!("{uri} {label}\n"));
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, content);
    }

    // Dolphin : uniquement si KDE a déjà créé son fichier de raccourcis.
    let path = kde_file();
    if let Ok(content) = std::fs::read_to_string(&path) {
        let marker = kde_marker(&uri);
        if let Some(end) = content.rfind("</xbel>").filter(|_| !content.contains(&marker)) {
            let entry = format!(
                " {marker}>\n  <title>{}</title>\n </bookmark>\n",
                glib::markup_escape_text(label)
            );
            let mut out = content.clone();
            out.insert_str(end, &entry);
            let _ = std::fs::write(&path, out);
        }
    }
}

pub fn remove(dir: &Path) {
    let uri = uri(dir);

    let path = gtk_file();
    if let Ok(content) = std::fs::read_to_string(&path) {
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

    let path = kde_file();
    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Some(out) = remove_xbel_entry(&content, &kde_marker(&uri)) {
            let _ = std::fs::write(&path, out);
        }
    }
}

/// Retire le bloc `<bookmark href="…"> … </bookmark>` (avec son indentation).
fn remove_xbel_entry(content: &str, marker: &str) -> Option<String> {
    let start = content.find(marker)?;
    let close = "</bookmark>";
    let end = start + content[start..].find(close)? + close.len();
    let line_start = content[..start].rfind('\n').map_or(0, |i| i + 1);
    let start = if content[line_start..start].trim().is_empty() { line_start } else { start };
    let end = if content[end..].starts_with('\n') { end + 1 } else { end };
    Some(format!("{}{}", &content[..start], &content[end..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xbel_remove() {
        let xbel = "<xbel>\n <bookmark href=\"file:///a\">\n  <title>A</title>\n </bookmark>\n <bookmark href=\"file:///b\">\n  <title>B</title>\n </bookmark>\n</xbel>\n";
        let out = remove_xbel_entry(xbel, "<bookmark href=\"file:///a\"").unwrap();
        assert_eq!(out, "<xbel>\n <bookmark href=\"file:///b\">\n  <title>B</title>\n </bookmark>\n</xbel>\n");
    }
}
