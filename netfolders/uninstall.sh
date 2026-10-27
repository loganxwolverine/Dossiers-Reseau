#!/bin/bash
# Désinstallation de NetFolders (les dossiers réseau déjà configurés restent en place)
set -euo pipefail
sudo rm -f /usr/local/bin/netfolders \
    /usr/local/share/applications/io.github.netfolders.NetFolders.desktop \
    /usr/share/polkit-1/actions/io.github.netfolders.NetFolders.policy
echo "NetFolders désinstallé."
