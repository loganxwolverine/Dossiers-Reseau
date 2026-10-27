#!/bin/bash
# Installation de NetFolders sous Fedora
#   ./install.sh            → utilise le programme précompilé (bin/netfolders) s'il est présent
#   ./install.sh --compiler → recompile depuis les sources (≈ 650 Mo d'outils à installer)
set -euo pipefail
cd "$(dirname "$0")"

COMPILER=0
[[ "${1:-}" == "--compiler" || ! -x bin/netfolders ]] && COMPILER=1

if [[ $COMPILER -eq 1 ]]; then
    # dnf télécharge dans /var/cache : on vérifie qu'il reste assez de place.
    LIBRE_MO=$(df -Pm /var/cache | awk 'NR==2 {print $4}')
    if (( LIBRE_MO < 1500 )); then
        echo "Espace disque insuffisant : ${LIBRE_MO} Mo libres, environ 1,5 Go nécessaires."
        echo "Libérez de la place (par ex. « sudo dnf clean all ») puis relancez."
        exit 1
    fi
    echo "==> Installation des outils de compilation (dnf)…"
    sudo dnf install -y --setopt=install_weak_deps=False \
        rust cargo gtk4-devel libadwaita-devel samba-client cifs-utils
    echo "==> Compilation…"
    cargo build --release
    BINAIRE=target/release/netfolders
else
    echo "==> Installation des dépendances (dnf)…"
    sudo dnf install -y samba-client cifs-utils gtk4 libadwaita
    BINAIRE=bin/netfolders
fi

echo "==> Installation…"
# Retire l'ancienne version (« Dossiers réseau » / monter-nas) si elle est présente.
sudo rm -f /usr/local/bin/monter-nas \
    /usr/local/share/applications/fr.monternas.MonterNas.desktop \
    /usr/share/polkit-1/actions/fr.monternas.MonterNas.policy
sudo install -Dm755 "$BINAIRE" /usr/local/bin/netfolders
sudo install -Dm644 data/io.github.netfolders.NetFolders.desktop /usr/local/share/applications/io.github.netfolders.NetFolders.desktop
sudo install -Dm644 data/io.github.netfolders.NetFolders.policy /usr/share/polkit-1/actions/io.github.netfolders.NetFolders.policy
# Étiquette SELinux correcte pour un exécutable copié dans /usr/local/bin
command -v restorecon &>/dev/null && sudo restorecon /usr/local/bin/netfolders || true

echo
echo "Terminé ! Lancez « NetFolders » depuis le menu des applications,"
echo "ou tapez : netfolders"
