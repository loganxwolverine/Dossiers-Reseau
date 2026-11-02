//! Lecture des montages réseau existants (/proc/self/mounts et /etc/fstab).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct NetMount {
    pub mount_dir: PathBuf,
    pub source: String,
    /// Actuellement monté.
    pub mounted: bool,
    /// Présent dans /etc/fstab (reconnecté au démarrage).
    pub persistent: bool,
    /// Options de montage (fstab en priorité, sinon celles du montage actif).
    pub options: String,
}

impl NetMount {
    fn option(&self, key: &str) -> Option<&str> {
        self.options.split(',').find_map(|o| o.strip_prefix(key)?.strip_prefix('='))
    }

    /// Les identifiants sont enregistrés dans un fichier (on peut les conserver).
    pub fn has_saved_credentials(&self) -> bool {
        self.option("credentials").is_some()
    }

    /// Accès sans compte.
    pub fn is_guest(&self) -> bool {
        self.options.split(',').any(|o| o == "guest") || (!self.has_saved_credentials() && self.username().is_none())
    }

    /// Nom d'utilisateur visible dans les options (absent si stocké dans un fichier d'identifiants).
    pub fn username(&self) -> Option<&str> {
        self.option("username").or_else(|| self.option("user"))
    }
}

/// Échappe un champ pour fstab (espaces, tabulations… en octal).
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            ' ' => out.push_str("\\040"),
            '\t' => out.push_str("\\011"),
            '\n' => out.push_str("\\012"),
            '\\' => out.push_str("\\134"),
            c => out.push(c),
        }
    }
    out
}

pub fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && (b'0'..=b'3').contains(&b[i + 1])
            && b[i + 2..i + 4].iter().all(|c| (b'0'..=b'7').contains(c))
        {
            out.push((b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0'));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn is_cifs(fstype: &str) -> bool {
    matches!(fstype, "cifs" | "smb3")
}

/// Entrées de type `(source, dossier, type, options)` d'un fichier au format fstab.
pub fn parse_table(content: &str) -> Vec<(String, String, String, String)> {
    content
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            (f.len() >= 3).then(|| {
                (
                    unescape(f[0]),
                    unescape(f[1]),
                    f[2].to_string(),
                    f.get(3).copied().unwrap_or_default().to_string(),
                )
            })
        })
        .collect()
}

pub fn is_mountpoint(dir: &Path) -> bool {
    let mounts = std::fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    parse_table(&mounts).iter().any(|(_, d, _, _)| Path::new(d) == dir)
}

pub fn is_cifs_mounted(dir: &Path) -> bool {
    let mounts = std::fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    parse_table(&mounts)
        .iter()
        .any(|(_, d, t, _)| is_cifs(t) && Path::new(d) == dir)
}

pub fn list() -> Vec<NetMount> {
    let mut map: BTreeMap<PathBuf, NetMount> = BTreeMap::new();
    let fstab = std::fs::read_to_string("/etc/fstab").unwrap_or_default();
    for (src, dir, t, opts) in parse_table(&fstab) {
        if is_cifs(&t) {
            map.insert(
                dir.clone().into(),
                NetMount { mount_dir: dir.into(), source: src, mounted: false, persistent: true, options: opts },
            );
        }
    }
    let mounts = std::fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    for (src, dir, t, opts) in parse_table(&mounts) {
        if is_cifs(&t) {
            map.entry(dir.clone().into())
                .or_insert(NetMount { mount_dir: dir.into(), source: src, mounted: false, persistent: false, options: opts })
                .mounted = true;
        }
    }
    map.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_roundtrip() {
        let s = "/home/paul/Mes Photos\\x";
        assert_eq!(escape(s), "/home/paul/Mes\\040Photos\\134x");
        assert_eq!(unescape(&escape(s)), s);
    }

    #[test]
    fn parse_fstab() {
        let t = "# commentaire\n//nas/a\\040b /home/p/a\\040b cifs credentials=/x,uid=1000 0 0\nUUID=1 / ext4 defaults 0 1\n";
        let e = parse_table(t);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].0, "//nas/a b");
        assert_eq!(e[0].1, "/home/p/a b");
        assert_eq!(e[0].2, "cifs");
    }
}
