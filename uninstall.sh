#!/bin/bash
# Désinstallation de NetFolders (installé avec install.sh).
# Les dossiers réseau déjà configurés restent en place : supprimez-les depuis
# l'application avant de la désinstaller si vous n'en voulez plus.
# (Si NetFolders a été installé avec un paquet .deb ou Arch, utilisez apt ou pacman.)
set -euo pipefail
ID_APP=io.github.netfolders.NetFolders
sudo rm -f /usr/local/bin/netfolders \
    "/usr/local/share/applications/$ID_APP.desktop" \
    "/usr/share/polkit-1/actions/$ID_APP.policy" \
    "/etc/polkit-1/actions/$ID_APP.policy" 2>/dev/null || true
rm -f "$HOME/.local/bin/netfolders" "$HOME/.local/share/applications/$ID_APP.desktop"
echo "NetFolders désinstallé."
