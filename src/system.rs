//! Détection de la distribution et gestion des paquets propres à chacune.

use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Distro {
    /// Fedora Workstation et dérivées classiques (dnf).
    Fedora,
    /// Fedora Atomic : Bazzite, Silverblue, Kinoite… (rpm-ostree, redémarrage nécessaire).
    FedoraAtomic,
    /// Debian, Ubuntu, Linux Mint, Pop!_OS, Zorin… (apt).
    Debian,
    /// Arch Linux, CachyOS, EndeavourOS, Manjaro… (pacman).
    Arch,
    /// SteamOS 3 : base Arch, mais système en lecture seule.
    SteamOs,
    Unknown,
}

impl Distro {
    pub fn detect() -> Distro {
        let os_release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
        Self::from_os_release(&os_release, Path::new("/run/ostree-booted").exists())
    }

    fn from_os_release(content: &str, ostree: bool) -> Distro {
        let field = |key: &str| {
            content
                .lines()
                .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
                .map(|v| v.trim_matches('"').to_lowercase())
                .unwrap_or_default()
        };
        let id = field("ID");
        let like = field("ID_LIKE");
        let is = |name: &str| id == name || like.split_whitespace().any(|l| l == name);

        if id == "steamos" {
            Distro::SteamOs
        } else if is("fedora") {
            if ostree { Distro::FedoraAtomic } else { Distro::Fedora }
        } else if is("debian") || is("ubuntu") {
            Distro::Debian
        } else if is("arch") {
            Distro::Arch
        } else {
            Distro::Unknown
        }
    }

    /// Nom du paquet qui fournit `smbclient` sur cette distribution.
    pub fn smbclient_package(self) -> &'static str {
        match self {
            Distro::Fedora | Distro::FedoraAtomic => "samba-client",
            _ => "smbclient",
        }
    }

    /// Une installation de paquets demande-t-elle un redémarrage ?
    pub fn needs_reboot_after_install(self) -> bool {
        self == Distro::FedoraAtomic
    }

    /// L'application peut-elle installer elle-même les paquets manquants ?
    pub fn can_install_packages(self) -> bool {
        matches!(self, Distro::Fedora | Distro::FedoraAtomic | Distro::Debian | Distro::Arch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection() {
        let d = |s: &str, ostree| Distro::from_os_release(s, ostree);
        assert_eq!(d("ID=fedora\n", false), Distro::Fedora);
        assert_eq!(d("ID=bazzite\nID_LIKE=\"fedora\"\n", true), Distro::FedoraAtomic);
        assert_eq!(d("ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n", false), Distro::Debian);
        assert_eq!(d("ID=ubuntu\nID_LIKE=debian\n", false), Distro::Debian);
        assert_eq!(d("ID=cachyos\nID_LIKE=arch\n", false), Distro::Arch);
        assert_eq!(d("ID=steamos\nID_LIKE=arch\n", false), Distro::SteamOs);
        assert_eq!(d("ID=opensuse-tumbleweed\n", false), Distro::Unknown);
    }
}
