#!/bin/bash
# Installation de NetFolders
#
# Distributions prises en charge :
#   - Fedora Workstation                       (dnf)
#   - Bazzite, Silverblue, Kinoite…            (rpm-ostree, redémarrage nécessaire)
#   - Debian 13+, Ubuntu 24.04+, Mint 22+…     (apt)
#   - Arch Linux, CachyOS, EndeavourOS…        (pacman)
#   - SteamOS 3 (expérimental)                 (pacman, système en lecture seule)
#
# Utilisation :
#   ./install.sh             utilise le programme précompilé (bin/netfolders)
#   ./install.sh --compiler  recompile depuis les sources
set -euo pipefail
cd "$(dirname "$0")"

ID_APP=io.github.netfolders.NetFolders
REDEMARRAGE=0

# ---------------------------------------------------------------------------
# 0. Détection de la distribution
# ---------------------------------------------------------------------------
. /etc/os-release
ID_LIKE=" ${ID_LIKE:-} "
if [[ "$ID" == "steamos" ]]; then
    DISTRO=steamos
elif [[ "$ID" == "fedora" || "$ID_LIKE" == *" fedora "* ]]; then
    if [[ -e /run/ostree-booted ]]; then DISTRO=atomic; else DISTRO=fedora; fi
elif [[ "$ID" == "debian" || "$ID" == "ubuntu" || "$ID_LIKE" == *" debian "* || "$ID_LIKE" == *" ubuntu "* ]]; then
    DISTRO=debian
elif [[ "$ID" == "arch" || "$ID_LIKE" == *" arch "* ]]; then
    DISTRO=arch
else
    DISTRO=inconnue
fi
echo "Système détecté : ${PRETTY_NAME:-$ID} ($DISTRO)"

# Paquets nécessaires au fonctionnement, selon la distribution
case "$DISTRO" in
    fedora|atomic)  PAQUETS=(samba-client cifs-utils gtk4 libadwaita) ;;
    debian)         PAQUETS=(smbclient cifs-utils libgtk-4-1 libadwaita-1-0 pkexec) ;;
    arch|steamos)   PAQUETS=(smbclient cifs-utils gtk4 libadwaita polkit) ;;
    *)              PAQUETS=() ;;
esac

est_installe() {
    case "$DISTRO" in
        fedora|atomic) rpm -q "$1" &>/dev/null ;;
        debian)        dpkg-query -W -f='${Status}' "$1" 2>/dev/null | grep -q "ok installed" ;;
        arch|steamos)  pacman -Q "$1" &>/dev/null ;;
        *)             return 0 ;;
    esac
}

installer_paquets() {
    case "$DISTRO" in
        fedora)
            sudo dnf install -y "$@" ;;
        atomic)
            sudo rpm-ostree install --idempotent --allow-inactive "$@"
            REDEMARRAGE=1 ;;
        debian)
            sudo apt-get update
            sudo apt-get install -y "$@" ;;
        arch)
            sudo pacman -S --needed --noconfirm "$@" ;;
        steamos)
            echo
            echo "SteamOS a un système en lecture seule. Pour ajouter ces paquets, le script va :"
            echo "  1. désactiver temporairement la protection (steamos-readonly disable),"
            echo "  2. installer les paquets avec pacman,"
            echo "  3. réactiver la protection."
            echo "Attention : une mise à jour de SteamOS peut les supprimer. Il suffira alors de"
            echo "relancer ./install.sh. (Un mot de passe doit être défini avec « passwd ».)"
            read -rp "Continuer ? (o/n) " REP
            [[ "$REP" =~ ^[oO]$ ]] || { echo "Installation annulée."; exit 1; }
            sudo steamos-readonly disable
            trap 'sudo steamos-readonly enable' EXIT
            sudo pacman-key --init
            sudo pacman-key --populate archlinux holo
            sudo pacman -Sy --needed --noconfirm "$@"
            sudo steamos-readonly enable
            trap - EXIT ;;
    esac
}

# ---------------------------------------------------------------------------
# 1. Programme : précompilé ou compilé ici
# ---------------------------------------------------------------------------
if [[ "${1:-}" == "--compiler" || ! -x bin/netfolders ]]; then
    case "$DISTRO" in
        fedora)
            sudo dnf install -y --setopt=install_weak_deps=False rust cargo gtk4-devel libadwaita-devel ;;
        atomic)
            # Le système est en lecture seule : compilation dans un conteneur Fedora jetable.
            command -v distrobox &>/dev/null || { echo "distrobox est introuvable."; exit 1; }
            distrobox list | grep -qw netfolders-build || \
                distrobox create --yes --name netfolders-build --image registry.fedoraproject.org/fedora:latest
            distrobox enter netfolders-build -- bash -c \
                "sudo dnf install -y --setopt=install_weak_deps=False rust cargo gtk4-devel libadwaita-devel && cargo build --release --locked"
            echo "(Le conteneur peut être supprimé avec : distrobox rm netfolders-build)" ;;
        debian)
            sudo apt-get update
            sudo apt-get install -y build-essential pkg-config libgtk-4-dev libadwaita-1-dev ;;
        arch)
            sudo pacman -S --needed --noconfirm base-devel rust gtk4 libadwaita ;;
        *)
            echo "Compilation automatique non prise en charge ici : utilisez le programme précompilé."
            exit 1 ;;
    esac
    if [[ "$DISTRO" != "atomic" ]]; then
        if ! command -v cargo &>/dev/null; then
            echo "Rust est introuvable. Installez-le avec rustup : https://rustup.rs"
            exit 1
        fi
        cargo build --release --locked || {
            echo
            echo "La compilation a échoué. Si votre version de Rust est trop ancienne,"
            echo "installez la dernière avec rustup : https://rustup.rs"
            exit 1
        }
    fi
    BINAIRE=target/release/netfolders
else
    BINAIRE=bin/netfolders
fi

# ---------------------------------------------------------------------------
# 2. Dépendances d'exécution
# ---------------------------------------------------------------------------
if [[ "$DISTRO" == "inconnue" ]]; then
    echo "Distribution non reconnue : vérifiez que smbclient, mount.cifs (cifs-utils),"
    echo "GTK 4, libadwaita (1.5 ou plus récent) et pkexec (polkit) sont installés."
else
    MANQUANTS=()
    for p in "${PAQUETS[@]}"; do
        est_installe "$p" || MANQUANTS+=("$p")
    done
    if [[ ${#MANQUANTS[@]} -gt 0 ]]; then
        echo "==> Paquets manquants : ${MANQUANTS[*]}"
        installer_paquets "${MANQUANTS[@]}"
    fi
fi

# NetFolders demande libadwaita 1.5 ou plus récent.
if [[ "$DISTRO" == "debian" ]]; then
    V=$(dpkg-query -W -f='${Version}' libadwaita-1-0 2>/dev/null || echo 0)
    if ! dpkg --compare-versions "$V" ge 1.5; then
        echo "Votre distribution fournit libadwaita $V, trop ancienne (1.5 minimum)."
        echo "NetFolders nécessite Debian 13, Ubuntu 24.04, Linux Mint 22 ou plus récent."
        exit 1
    fi
fi

# ---------------------------------------------------------------------------
# 3. Installation des fichiers
# ---------------------------------------------------------------------------
echo "==> Installation…"
# Retire l'ancienne version (« Dossiers réseau » / monter-nas) si elle est présente.
sudo rm -f /usr/local/bin/monter-nas \
    /usr/local/share/applications/fr.monternas.MonterNas.desktop \
    /usr/share/polkit-1/actions/fr.monternas.MonterNas.policy 2>/dev/null || true

# Programme : /usr/local/bin si possible (sur Bazzite c'est un lien vers /var/usrlocal),
# sinon ~/.local/bin (SteamOS, dont /usr est en lecture seule).
if sudo install -Dm755 "$BINAIRE" /usr/local/bin/netfolders 2>/dev/null; then
    CHEMIN=$(realpath /usr/local/bin/netfolders)
    command -v restorecon &>/dev/null && sudo restorecon "$CHEMIN" || true
else
    install -Dm755 "$BINAIRE" "$HOME/.local/bin/netfolders"
    CHEMIN=$(realpath "$HOME/.local/bin/netfolders")
fi

# Raccourci du menu des applications (chemin complet du programme dans Exec=).
# Sur les systèmes en lecture seule, le dossier personnel est le plus sûr.
if [[ "$DISTRO" != "atomic" && "$DISTRO" != "steamos" ]]; then
    DESKTOP=/usr/local/share/applications/$ID_APP.desktop
    sudo mkdir -p "$(dirname "$DESKTOP")"
    sed "s#^Exec=.*#Exec=$CHEMIN#" "data/$ID_APP.desktop" | sudo tee "$DESKTOP" > /dev/null
else
    DESKTOP=$HOME/.local/share/applications/$ID_APP.desktop
    mkdir -p "$(dirname "$DESKTOP")"
    sed "s#^Exec=.*#Exec=$CHEMIN#" "data/$ID_APP.desktop" > "$DESKTOP"
fi

# Règle polkit : message clair dans la fenêtre de mot de passe administrateur.
# /usr/share est en lecture seule sur Bazzite et SteamOS : on utilise alors /etc.
if sudo test -w /usr/share/polkit-1/actions; then
    DOSSIER_POLKIT=/usr/share/polkit-1/actions
else
    DOSSIER_POLKIT=/etc/polkit-1/actions
    sudo mkdir -p "$DOSSIER_POLKIT"
fi
sed "s#/usr/local/bin/netfolders#$CHEMIN#" "data/$ID_APP.policy" \
    | sudo tee "$DOSSIER_POLKIT/$ID_APP.policy" > /dev/null

# Rafraîchit les menus (KDE / autres bureaux), si les outils sont présents.
for k in kbuildsycoca6 kbuildsycoca5; do
    command -v "$k" &>/dev/null && { "$k" &>/dev/null || true; break; }
done
command -v update-desktop-database &>/dev/null && \
    update-desktop-database "$(dirname "$DESKTOP")" &>/dev/null || true

echo
if [[ $REDEMARRAGE -eq 1 ]]; then
    echo "Terminé ! REDÉMARREZ l'ordinateur pour activer les paquets ajoutés,"
    echo "puis lancez « NetFolders » depuis le menu des applications."
else
    echo "Terminé ! Lancez « NetFolders » depuis le menu des applications,"
    echo "ou tapez : $CHEMIN"
fi
