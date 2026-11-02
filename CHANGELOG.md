# Changelog

All notable changes to NetFolders are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.3.0] - 2026-11-02

### Added
- Debian, Ubuntu, Linux Mint and derivatives: `.deb` package and `install.sh` support (apt).
- Arch Linux, CachyOS, EndeavourOS and derivatives: `PKGBUILD`, prebuilt package and `install.sh` support (pacman).
- SteamOS 3 (experimental): `install.sh` support, including the read-only system.
- Automatic builds of the tarball, `.deb` and Arch package with GitHub Actions.
- English README, MIT license, issue templates.

### Changed
- The missing-package installer now uses the right package manager for each distribution.

## [0.2.0] - 2026-10-30

### Added
- Bazzite and Fedora Atomic support (rpm-ostree, `/var/home` paths, `/etc/polkit-1/actions`).
- Shortcuts in Dolphin (KDE) in addition to GNOME Files.
- Edit existing network folders, with automatic rollback if the new settings fail.
- Prebuilt binary: no need to install a Rust toolchain.

### Changed
- Project renamed from « Dossiers réseau » / `monter-nas` to **NetFolders**.

## [0.1.0] - 2026-10-27

### Added
- First graphical version (GTK4 / libadwaita), replacing the original Bash script.
- 4-step wizard: NAS address, share, folder, options.
- Optional automatic reconnection at boot (`/etc/fstab` with `x-systemd.automount`, `nofail`).
- List, open, disconnect and remove existing network folders.
