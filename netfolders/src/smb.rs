//! Exploration d'un NAS via `smbclient` (exécuté en tant qu'utilisateur normal).

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use regex::Regex;

#[derive(Clone, Debug, Default)]
pub struct Credentials {
    /// `None` = accès invité.
    pub username: Option<String>,
    pub password: String,
}

impl Credentials {
    /// Contenu d'un fichier d'identifiants, au format commun à
    /// `smbclient -A` et `mount.cifs credentials=`.
    /// Accepte « DOMAINE\utilisateur » ou « utilisateur@domaine ».
    pub fn file_content(&self) -> String {
        let user = self.username.as_deref().unwrap_or_default();
        let (domain, user) = match user.split_once('\\') {
            Some((d, u)) => (Some(d), u),
            None => (None, user),
        };
        let mut s = format!("username={user}\npassword={}\n", self.password);
        if let Some(d) = domain {
            s.push_str(&format!("domain={d}\n"));
        }
        s
    }
}

#[derive(Clone, Debug)]
pub struct Share {
    pub name: String,
    pub comment: String,
}

/// Arguments d'authentification pour smbclient. Le mot de passe passe par un
/// fichier temporaire en 0600 (jamais sur la ligne de commande, visible dans `ps`).
struct Auth {
    args: Vec<String>,
    _file: Option<tempfile::NamedTempFile>,
}

fn auth(creds: &Credentials) -> Result<Auth, String> {
    if creds.username.is_none() {
        return Ok(Auth { args: vec!["-N".into()], _file: None });
    }
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let mut f = tempfile::Builder::new()
        .prefix("netfolders-")
        .tempfile_in(dir)
        .map_err(|e| format!("Impossible de créer un fichier temporaire : {e}"))?;
    f.write_all(creds.file_content().as_bytes())
        .and_then(|_| f.flush())
        .map_err(|e| format!("Impossible d'écrire un fichier temporaire : {e}"))?;
    Ok(Auth {
        args: vec!["-A".into(), f.path().to_string_lossy().into_owned()],
        _file: Some(f),
    })
}

fn smbclient(args: &[String]) -> Result<(bool, String, String), String> {
    let out = Command::new("smbclient")
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| format!("Impossible de lancer smbclient ({e}). Le paquet samba-client est-il installé ?"))?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

/// Traduit les codes d'erreur Samba en messages compréhensibles.
pub fn explain(output: &str) -> String {
    let has = |k: &str| output.contains(k);
    if has("NT_STATUS_LOGON_FAILURE") || has("NT_STATUS_WRONG_PASSWORD") {
        "Nom d'utilisateur ou mot de passe incorrect.".into()
    } else if has("NT_STATUS_ACCOUNT_DISABLED") || has("NT_STATUS_ACCOUNT_LOCKED_OUT") {
        "Ce compte est désactivé ou verrouillé sur le NAS.".into()
    } else if has("NT_STATUS_ACCESS_DENIED") {
        "Accès refusé : ce compte n'a pas le droit d'ouvrir cet emplacement.".into()
    } else if has("NT_STATUS_BAD_NETWORK_NAME") {
        "Ce partage n'existe pas (ou plus) sur le NAS.".into()
    } else if has("NT_STATUS_OBJECT_NAME_NOT_FOUND") || has("NT_STATUS_OBJECT_PATH_NOT_FOUND") {
        "Ce dossier n'existe plus sur le NAS.".into()
    } else if has("NT_STATUS_HOST_UNREACHABLE")
        || has("NT_STATUS_IO_TIMEOUT")
        || has("NT_STATUS_CONNECTION_REFUSED")
        || has("NT_STATUS_NETWORK_UNREACHABLE")
        || has("NT_STATUS_UNSUCCESSFUL")
        || has("Connection to")
    {
        "Impossible de joindre le NAS à cette adresse.\n\n\
         Vérifiez que l'adresse est correcte, que le NAS est allumé et que \
         cet ordinateur est sur le même réseau."
            .into()
    } else if has("NT_STATUS_BAD_NETWORK_PATH") || has("resolve") {
        "Ce nom de NAS est introuvable sur le réseau. Essayez plutôt avec son adresse IP.".into()
    } else if has("NT_STATUS_CONNECTION_DISCONNECTED")
        || has("NT_STATUS_INVALID_NETWORK_RESPONSE")
        || has("protocol negotiation failed")
    {
        "Le NAS a refusé la connexion. Il utilise peut-être l'ancien protocole SMB1, \
         désactivé par défaut sous Fedora pour des raisons de sécurité : activez \
         SMB2 ou SMB3 dans les réglages du NAS."
            .into()
    } else {
        let tail: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
        let tail = tail[tail.len().saturating_sub(4)..].join("\n");
        format!("Erreur inattendue :\n{tail}")
    }
}

/// Liste les partages « disque » visibles (les partages administratifs `xxx$` sont masqués).
pub fn list_shares(host: &str, creds: &Credentials) -> Result<Vec<Share>, String> {
    let a = auth(creds)?;
    let mut args = vec!["-L".to_string(), format!("//{host}"), "-g".into()];
    args.extend(a.args.iter().cloned());
    let (ok, stdout, stderr) = smbclient(&args)?;

    let mut shares: Vec<Share> = stdout
        .lines()
        .filter_map(|l| {
            let mut f = l.splitn(3, '|');
            match (f.next(), f.next(), f.next()) {
                (Some("Disk"), Some(name), comment) if !name.ends_with('$') => Some(Share {
                    name: name.to_string(),
                    comment: comment.unwrap_or_default().to_string(),
                }),
                _ => None,
            }
        })
        .collect();
    shares.sort_by_key(|s| s.name.to_lowercase());

    // smbclient renvoie parfois une erreur (listage SMB1 du groupe de travail)
    // alors que la liste des partages a bien été obtenue.
    if shares.is_empty() && (!ok || stdout.contains("NT_STATUS") || stderr.contains("NT_STATUS")) {
        return Err(explain(&format!("{stdout}\n{stderr}")));
    }
    Ok(shares)
}

fn ls_line_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Ex. : "  Mes Photos                          D        0  Mon Oct 27 10:00:00 2026"
    RE.get_or_init(|| {
        Regex::new(r"^  (.+?)\s+([A-Z]+)\s+\d+\s+\w{3} \w{3}\s+\d{1,2} \d{2}:\d{2}:\d{2} \d{4}$").unwrap()
    })
}

/// Liste les sous-dossiers (non cachés) de `path` dans le partage `share`.
pub fn list_dirs(host: &str, share: &str, path: &[String], creds: &Credentials) -> Result<Vec<String>, String> {
    let a = auth(creds)?;
    let mut args = vec![format!("//{host}/{share}")];
    args.extend(a.args.iter().cloned());
    if !path.is_empty() {
        // -D évite d'avoir à échapper les espaces / points-virgules dans une commande -c.
        args.push("-D".into());
        args.push(path.join("/"));
    }
    args.push("-c".into());
    args.push("ls".into());
    let (ok, stdout, stderr) = smbclient(&args)?;
    if !ok || stdout.contains("NT_STATUS_") {
        return Err(explain(&format!("{stdout}\n{stderr}")));
    }

    let mut dirs: Vec<String> = stdout
        .lines()
        .filter_map(|l| ls_line_regex().captures(l))
        .filter(|c| c[2].contains('D') && !c[2].contains('H'))
        .map(|c| c[1].to_string())
        .filter(|n| n != "." && n != "..")
        .collect();
    dirs.sort_by_key(|d| d.to_lowercase());
    Ok(dirs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ls_with_spaces() {
        let re = ls_line_regex();
        let c = re.captures("  Mes Photos 2024                    D        0  Mon Oct 27 10:00:00 2026").unwrap();
        assert_eq!(&c[1], "Mes Photos 2024");
        assert_eq!(&c[2], "D");
        let c = re.captures("  facture.pdf                         A   123456  Tue Jan  2 08:05:00 2024").unwrap();
        assert_eq!(&c[1], "facture.pdf");
        assert_eq!(&c[2], "A");
        assert!(re.captures("\t\t1234 blocks of size 4096. 12 blocks available").is_none());
    }

    #[test]
    fn credentials_with_domain() {
        let c = Credentials { username: Some("MAISON\\paul".into()), password: "a,b".into() };
        assert_eq!(c.file_content(), "username=paul\npassword=a,b\ndomain=MAISON\n");
    }
}
