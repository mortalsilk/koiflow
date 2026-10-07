#!/bin/sh
set -eu

cargo build --release
install -Dm755 target/release/koiflow "$HOME/.local/bin/koiflow"
install -Dm644 packaging/koiflow.desktop "$HOME/.local/share/applications/koiflow.desktop"
install -Dm644 packaging/koiflow.svg "$HOME/.local/share/icons/hicolor/scalable/apps/koiflow.svg"
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$HOME/.local/share/applications" || true
printf '%s\n' 'KoiFlow installed for the current user. No root access was used.'
