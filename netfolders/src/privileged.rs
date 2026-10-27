//! Opérations nécessitant les droits administrateur.
//!
//! L'interface relance ce même exécutable via `pkexec netfolders --helper` et lui
//! transmet la demande en JSON sur l'entrée standard (le mot de passe n'apparaît
//! donc jamais dans la liste des processus).

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::mounts;
use crate::smb::Credentials;

const CRED_DIR: &str = "/etc/samba/credentials";
const FSTAB: &str = "/etc/fstab";
const BACKUP: &str = "/etc/fstab.netfolders.bak";
const MARK: &str = "# Ajouté par NetFolders";
/// Commentaire laissé par l'ancienne version (« monter-nas »).
const OLD_MARK: &str = "# Ajouté par monter-nas";

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Request {
    Mount {
        source: String,
        mount_dir: String,
        uid: u32,
        gid: u32,
        username: Option<String>,
        password: String,
        persist: bool,
    },
    Unmount {
        mount_dir: String,
        /// Retire aussi l'entrée de /etc/fstab et le fichier d'identifiants.
        forget: bool,
    },
    /// Remplace un dossier réseau existant par une nouvelle configuration.
    Modify {
        old_mount_dir: String,
        source: String,
        mount_dir: String,
        uid: u32,
        gid: u32,
        credentials: CredChoice,
        persist: bool,
    },
    InstallPackages {
        packages: Vec<String>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CredChoice {
    /// Conserver les identifiants actuellement enregistrés.
    Keep,
    Guest,
    User { username: String, password: String },
}

// ---------------------------------------------------------------------------
// Côté interface (utilisateur normal)
// ---------------------------------------------------------------------------

/// Exécute la demande avec les droits administrateur. Bloquant : à appeler hors du thread GTK.
pub fn run(req: &Request) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| format!("Exécutable introuvable : {e}"))?;
    let json = serde_json::to_vec(req).map_err(|e| e.to_string())?;
    let mut child = Command::new("pkexec")
        .arg(exe)
        .arg("--helper")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Impossible de lancer pkexec : {e}"))?;
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(&json)
        .map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    match out.status.code() {
        Some(0) => Ok(stdout),
        Some(126) | Some(127) => Err("Authentification annulée ou refusée.".into()),
        _ if !stderr.is_empty() => Err(stderr),
        _ => Err("Erreur inconnue lors de l'opération administrateur.".into()),
    }
}

// ---------------------------------------------------------------------------
// Côté assistant root (`netfolders --helper`)
// ---------------------------------------------------------------------------

pub fn helper_main() -> i32 {
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("Ce mode doit être lancé via pkexec.");
        return 1;
    }
    let mut input = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut input) {
        eprintln!("{e}");
        return 1;
    }
    let result = serde_json::from_str::<Request>(&input)
        .map_err(|e| format!("Demande invalide : {e}"))
        .and_then(|req| match req {
            Request::Mount { source, mount_dir, uid, gid, username, password, persist } => {
                let creds = Credentials { username, password };
                do_mount(&source, Path::new(&mount_dir), uid, gid, &creds, persist)
            }
            Request::Unmount { mount_dir, forget } => do_unmount(Path::new(&mount_dir), forget),
            Request::Modify { old_mount_dir, source, mount_dir, uid, gid, credentials, persist } => do_modify(
                Path::new(&old_mount_dir),
                &source,
                Path::new(&mount_dir),
                uid,
                gid,
                credentials,
                persist,
            ),
            Request::InstallPackages { packages } => install_packages(&packages),
        });
    match result {
        Ok(msg) => {
            println!("{msg}");
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

fn no_control_chars(s: &str, what: &str) -> Result<(), String> {
    if s.chars().any(char::is_control) {
        return Err(format!("{what} contient des caractères invalides."));
    }
    Ok(())
}

fn check_dir(dir: &Path) -> Result<String, String> {
    let s = dir.to_str().ok_or("Chemin non UTF-8.")?;
    no_control_chars(s, "Le chemin local")?;
    if !dir.is_absolute() || !dir.is_dir() {
        return Err(format!("Le dossier {s} n'existe pas."));
    }
    Ok(s.to_string())
}

/// Nom de fichier d'identifiants dérivé du point de montage : /home/paul/NAS → home-paul-NAS.cred
fn cred_file_for(dir: &str) -> PathBuf {
    let name: String = dir
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '.' { c } else { '-' })
        .collect();
    Path::new(CRED_DIR).join(format!("{}.cred", name.trim_matches('-')))
}

fn write_secret(path: &Path, content: &str) -> Result<(), String> {
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("Impossible d'écrire {} : {e}", path.display()))?;
    f.write_all(content.as_bytes()).map_err(|e| e.to_string())
}

fn explain_mount_error(out: &str) -> String {
    let code = |n: &str| out.contains(&format!("error({n})"));
    if code("13") {
        "Le NAS a refusé les identifiants (nom d'utilisateur ou mot de passe incorrect, \
         ou accès invité non autorisé)."
            .into()
    } else if code("2") {
        "Ce dossier est introuvable sur le NAS.".into()
    } else if code("112") || code("113") || code("115") || code("110") || code("101") {
        "Le NAS ne répond pas. Vérifiez qu'il est allumé et joignable.".into()
    } else if code("95") {
        "Le NAS ne prend pas en charge une version récente du protocole SMB. \
         Activez SMB2 ou SMB3 dans ses réglages."
            .into()
    } else {
        format!("Le montage a échoué :\n{}", out.lines().next().unwrap_or(out))
    }
}

fn do_mount(source: &str, dir: &Path, uid: u32, gid: u32, creds: &Credentials, persist: bool) -> Result<String, String> {
    no_control_chars(source, "L'adresse du dossier réseau")?;
    if !source.starts_with("//") {
        return Err("Adresse de dossier réseau invalide.".into());
    }
    let dir_s = check_dir(dir)?;
    // pkexec indique quel utilisateur a demandé l'opération : on ne monte que pour lui.
    if let Some(caller) = std::env::var("PKEXEC_UID").ok().and_then(|v| v.parse::<u32>().ok()) {
        if caller != uid {
            return Err("Utilisateur incohérent.".into());
        }
    }
    if mounts::is_mountpoint(dir) {
        return Err(format!("Un dossier est déjà monté sur {dir_s}."));
    }

    let mut messages = Vec::new();
    let base_opts = format!("uid={uid},gid={gid},iocharset=utf8,file_mode=0644,dir_mode=0755");

    // Fichier d'identifiants : permanent si persistant, sinon temporaire (supprimé après montage).
    let mut _tmp = None;
    let auth_opt = if creds.username.is_some() {
        let path = if persist {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(CRED_DIR)
                .map_err(|e| format!("Impossible de créer {CRED_DIR} : {e}"))?;
            let p = cred_file_for(&dir_s);
            write_secret(&p, &creds.file_content())?;
            p
        } else {
            let t = tempfile::Builder::new()
                .prefix("netfolders-")
                .tempfile_in("/run")
                .map_err(|e| e.to_string())?;
            write_secret(t.path(), &creds.file_content())?;
            let p = t.path().to_path_buf();
            _tmp = Some(t);
            p
        };
        format!("credentials={}", path.display())
    } else {
        "guest".to_string()
    };

    let opts = format!("{auth_opt},{base_opts}");
    let out = Command::new("mount")
        .args(["-t", "cifs", source, &dir_s, "-o", &opts])
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| format!("Impossible de lancer mount : {e}"))?;
    if !out.status.success() {
        if persist && creds.username.is_some() {
            let _ = fs::remove_file(cred_file_for(&dir_s));
        }
        let text = format!("{}{}", String::from_utf8_lossy(&out.stderr), String::from_utf8_lossy(&out.stdout));
        return Err(explain_mount_error(&text));
    }
    messages.push(format!("Monté sur {dir_s}."));

    if persist {
        let fstab = fs::read_to_string(FSTAB).map_err(|e| format!("Lecture de {FSTAB} impossible : {e}"))?;
        let exists = mounts::parse_table(&fstab).iter().any(|(_, d, _, _)| Path::new(d) == dir);
        if exists {
            messages.push(format!("Une entrée pour ce dossier existe déjà dans {FSTAB} : elle n'a pas été modifiée."));
        } else {
            let line = format!(
                "{} {} cifs {opts},_netdev,nofail,x-systemd.automount,x-systemd.mount-timeout=30 0 0\n",
                mounts::escape(source),
                mounts::escape(&dir_s),
            );
            let _ = fs::copy(FSTAB, BACKUP);
            let mut f = fs::OpenOptions::new()
                .append(true)
                .open(FSTAB)
                .map_err(|e| format!("Écriture dans {FSTAB} impossible : {e}"))?;
            let prefix = if fstab.is_empty() || fstab.ends_with('\n') { "" } else { "\n" };
            f.write_all(format!("{prefix}{MARK}\n{line}").as_bytes())
                .map_err(|e| e.to_string())?;
            let _ = Command::new("systemctl").arg("daemon-reload").output();
            messages.push("Montage automatique au démarrage activé.".into());
        }
    }
    Ok(messages.join("\n"))
}

fn systemd_unit(dir: &str, suffix: &str) -> Option<String> {
    let out = Command::new("systemd-escape")
        .args(["--path", &format!("--suffix={suffix}"), dir])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn do_unmount(dir: &Path, forget: bool) -> Result<String, String> {
    let dir_s = dir.to_str().ok_or("Chemin non UTF-8.")?.to_string();
    no_control_chars(&dir_s, "Le chemin local")?;

    if forget {
        // Arrête l'automontage systemd, sinon le dossier serait remonté au prochain accès.
        for suffix in ["automount", "mount"] {
            if let Some(unit) = systemd_unit(&dir_s, suffix) {
                let _ = Command::new("systemctl").args(["stop", &unit]).output();
            }
        }
    }

    if mounts::is_cifs_mounted(dir) {
        let out = Command::new("umount")
            .arg(&dir_s)
            .env("LC_ALL", "C")
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(if err.contains("busy") {
                "Le dossier est en cours d'utilisation. Fermez les fenêtres et \
                 applications qui l'utilisent, puis réessayez."
                    .into()
            } else {
                format!("Le démontage a échoué :\n{}", err.trim())
            });
        }
    }

    if forget {
        let fstab = fs::read_to_string(FSTAB).map_err(|e| e.to_string())?;
        let mut kept = Vec::new();
        let mut cred_files = Vec::new();
        let mut removed = false;
        let lines: Vec<&str> = fstab.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let entry = mounts::parse_table(line);
            let matches = entry
                .first()
                .is_some_and(|(_, d, t, _)| Path::new(d) == dir && (t == "cifs" || t == "smb3"));
            if matches {
                removed = true;
                for opt in entry[0].3.split(',') {
                    if let Some(p) = opt.strip_prefix("credentials=") {
                        cred_files.push(PathBuf::from(p));
                    }
                }
                // Retire aussi notre commentaire juste au-dessus.
                if i > 0 && (lines[i - 1] == MARK || lines[i - 1] == OLD_MARK) {
                    kept.pop();
                }
            } else {
                kept.push(*line);
            }
        }
        if removed {
            let _ = fs::copy(FSTAB, BACKUP);
            let mut content = kept.join("\n");
            content.push('\n');
            fs::write(FSTAB, content).map_err(|e| format!("Écriture dans {FSTAB} impossible : {e}"))?;
            for p in cred_files.iter().filter(|p| p.starts_with(CRED_DIR)) {
                let _ = fs::remove_file(p);
            }
            let _ = Command::new("systemctl").arg("daemon-reload").output();
        }
    }
    Ok("Dossier réseau déconnecté.".into())
}

/// Lit un fichier d'identifiants (format mount.cifs) : username / password / domain.
fn read_cred_file(path: &Path) -> Result<Credentials, String> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("Lecture des identifiants enregistrés impossible ({}) : {e}", path.display()))?;
    let (mut user, mut pass, mut domain) = (None, String::new(), None);
    for line in content.lines() {
        if let Some((k, v)) = line.split_once('=') {
            match k.trim() {
                "username" | "user" => user = Some(v.to_string()),
                "password" | "pass" => pass = v.to_string(),
                "domain" | "dom" => domain = Some(v.to_string()),
                _ => {}
            }
        }
    }
    let username = user.map(|u| match &domain {
        Some(d) => format!("{d}\\{u}"),
        None => u,
    });
    Ok(Credentials { username, password: pass })
}

/// Identifiants actuellement utilisés par un dossier réseau (pour « conserver »).
fn current_credentials(dir: &Path) -> Result<Credentials, String> {
    let fstab = fs::read_to_string(FSTAB).unwrap_or_default();
    let mounts_now = fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    let find = |table: &str| {
        mounts::parse_table(table)
            .into_iter()
            .find(|(_, d, t, _)| Path::new(d) == dir && (t == "cifs" || t == "smb3"))
            .map(|e| e.3)
    };
    let opts = find(&fstab).or_else(|| find(&mounts_now)).unwrap_or_default();
    let opts: Vec<&str> = opts.split(',').collect();
    if let Some(p) = opts.iter().find_map(|o| o.strip_prefix("credentials=")) {
        return read_cred_file(Path::new(p));
    }
    if opts.iter().any(|o| o.starts_with("username=") || o.starts_with("user=")) {
        return Err("Le mot de passe actuel n'est pas enregistré sur cet ordinateur : \
                    saisissez à nouveau les identifiants."
            .into());
    }
    Ok(Credentials::default())
}

fn do_modify(
    old_dir: &Path,
    source: &str,
    new_dir: &Path,
    uid: u32,
    gid: u32,
    choice: CredChoice,
    persist: bool,
) -> Result<String, String> {
    let creds = match choice {
        CredChoice::Keep => current_credentials(old_dir)?,
        CredChoice::Guest => Credentials::default(),
        CredChoice::User { username, password } => Credentials { username: Some(username), password },
    };

    // Sauvegarde de l'état actuel pour pouvoir revenir en arrière en cas d'échec.
    let old_fstab = fs::read_to_string(FSTAB).map_err(|e| e.to_string())?;
    let old_creds: Vec<(PathBuf, String)> = mounts::parse_table(&old_fstab)
        .iter()
        .filter(|(_, d, _, _)| Path::new(d) == old_dir)
        .flat_map(|(_, _, _, o)| o.split(',').filter_map(|x| x.strip_prefix("credentials=")).map(PathBuf::from).collect::<Vec<_>>())
        .filter_map(|p| fs::read_to_string(&p).ok().map(|c| (p, c)))
        .collect();
    let was_mounted = mounts::is_cifs_mounted(old_dir);

    do_unmount(old_dir, true)?;

    match do_mount(source, new_dir, uid, gid, &creds, persist) {
        Ok(msg) => Ok(format!("Modifications enregistrées.\n{msg}")),
        Err(e) => {
            // Restauration de l'ancienne configuration.
            let _ = fs::write(FSTAB, &old_fstab);
            for (p, c) in &old_creds {
                let _ = write_secret(p, c);
            }
            let _ = Command::new("systemctl").arg("daemon-reload").output();
            if was_mounted {
                let _ = Command::new("mount").arg(old_dir).output();
            }
            let mut msg = format!("{e}\n\nAucune modification n'a été appliquée : l'ancienne configuration a été restaurée.");
            if was_mounted && !mounts::is_cifs_mounted(old_dir) {
                msg.push_str(" L'ancien dossier n'a cependant pas pu être reconnecté.");
            }
            Err(msg)
        }
    }
}

fn install_packages(packages: &[String]) -> Result<String, String> {
    const ALLOWED: [&str; 2] = ["samba-client", "cifs-utils"];
    if packages.is_empty() || packages.iter().any(|p| !ALLOWED.contains(&p.as_str())) {
        return Err("Paquet non autorisé.".into());
    }
    let out = Command::new("dnf")
        .args(["install", "-y"])
        .args(packages)
        .output()
        .map_err(|e| format!("Impossible de lancer dnf : {e}"))?;
    if out.status.success() {
        Ok("Paquets installés.".into())
    } else {
        Err(format!("L'installation a échoué :\n{}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cred_file_name() {
        assert_eq!(cred_file_for("/home/paul/Mes Photos"), Path::new("/etc/samba/credentials/home-paul-Mes-Photos.cred"));
    }
}
