#!/bin/bash
# Construit le paquet .deb de NetFolders (Debian 13+, Ubuntu 24.04+, Linux Mint 22+…).
#   packaging/debian/build-deb.sh <binaire> <version> <dossier de sortie>
set -euo pipefail
BINAIRE=$1
VERSION=$2
SORTIE=$3
RACINE=$(cd "$(dirname "$0")/../.." && pwd)
ID_APP=io.github.netfolders.NetFolders

STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
PKG="$STAGE/netfolders_${VERSION}_amd64"

install -Dm755 "$BINAIRE" "$PKG/usr/bin/netfolders"
install -Dm644 "$RACINE/data/$ID_APP.desktop" "$PKG/usr/share/applications/$ID_APP.desktop"
sed 's#/usr/local/bin/netfolders#/usr/bin/netfolders#' "$RACINE/data/$ID_APP.policy" \
    | install -Dm644 /dev/stdin "$PKG/usr/share/polkit-1/actions/$ID_APP.policy"
install -Dm644 "$RACINE/LICENSE" "$PKG/usr/share/doc/netfolders/copyright"

TAILLE=$(du -sk "$PKG/usr" | cut -f1)
mkdir -p "$PKG/DEBIAN"
cat > "$PKG/DEBIAN/control" <<CONTROL
Package: netfolders
Version: $VERSION
Section: utils
Priority: optional
Architecture: amd64
Installed-Size: $TAILLE
Depends: libc6 (>= 2.39), libgtk-4-1 (>= 4.12), libadwaita-1-0 (>= 1.5), smbclient, cifs-utils, pkexec | policykit-1
Maintainer: loganxwolverine <loganxwolverine@users.noreply.github.com>
Homepage: https://github.com/loganxwolverine/NetFolders
Description: Mount and manage NAS network folders (SMB/CIFS)
 NetFolders is a GTK4/libadwaita application to mount the shared folders
 of a NAS (SMB/CIFS) without using a terminal: step-by-step wizard,
 optional automatic reconnection at boot (/etc/fstab), edit and removal
 of existing network folders.
CONTROL

mkdir -p "$SORTIE"
dpkg-deb --root-owner-group --build "$PKG" "$SORTIE/netfolders_${VERSION}_amd64.deb"
