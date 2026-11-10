#!/usr/bin/env bash
# build-appimage.sh — empacota o binário musl estático em um AppImage 1-clique.
#
# Uso:  ./dist/build-appimage.sh [caminho-do-binário]
# Requisitos: binário x86_64-unknown-linux-musl já compilado + curl.
# O appimagetool é baixado automaticamente (modo extract-and-run, sem FUSE).
set -euo pipefail

ROOT="$(cd "$(dirname "${0}")/.." && pwd)"
BIN="${1:-$ROOT/target/x86_64-unknown-linux-musl/release/ps2-manager}"
APPDIR="$ROOT/build/AppDir"
OUT="$ROOT/build/ps2-manager-x86_64.AppImage"
TOOL="${APPIMAGETOOL:-/tmp/appimagetool-x86_64.AppImage}"

if [ ! -x "$BIN" ]; then
    echo "✗ binário não encontrado: $BIN" >&2
    echo "  compile primeiro: cargo build --release --target x86_64-unknown-linux-musl" >&2
    exit 1
fi

rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" "$APPDIR/usr/share/icons/hicolor/256x256/apps"

cp "$BIN" "$APPDIR/usr/bin/ps2-manager"
cp "$ROOT/dist/AppRun" "$APPDIR/AppRun"
chmod +x "$APPDIR/AppRun" "$APPDIR/usr/bin/ps2-manager"
cp "$ROOT/dist/ps2-manager.desktop" "$APPDIR/ps2-manager.desktop"
cp "$ROOT/dist/ps2-manager.desktop" "$APPDIR/usr/share/applications/ps2-manager.desktop"

# ícone embutido (se existir); senão usa ícone de tema no .desktop
if [ -f "$ROOT/dist/icon.png" ]; then
    cp "$ROOT/dist/icon.png" "$APPDIR/ps2-manager.png"
    cp "$ROOT/dist/icon.png" "$APPDIR/usr/share/icons/hicolor/256x256/apps/ps2-manager.png"
    cp "$ROOT/dist/icon.png" "$APPDIR/.DirIcon"
else
    echo "⚠ dist/icon.png ausente — usando ícone de tema (input-gaming)"
    sed -i 's/^Icon=.*/Icon=input-gaming/' "$APPDIR/ps2-manager.desktop"
    sed -i 's/^Icon=.*/Icon=input-gaming/' "$APPDIR/usr/share/applications/ps2-manager.desktop"
fi

# AppStream metadata (opcional, melhora integração com lojas)
mkdir -p "$APPDIR/usr/share/metainfo"
cat > "$APPDIR/usr/share/metainfo/ps2-manager.appdata.xml" <<'XML'
<?xml version="1.0" encoding="UTF-8"?>
<component type="desktop-application">
  <id>ps2-manager</id>
  <name>PS2 Manager</name>
  <summary>Gerenciador nativo Linux para coleções PlayStation 2 / OPL</summary>
  <description>
    <p>ISO9660/ZSO, Smart Shrink, staging em SSD, gravação validada em mídias
    externas (anti-desastre), deploy de Boot Kit e dashboard TUI com mouse.</p>
  </description>
  <launchable type="desktop-id">ps2-manager.desktop</launchable>
  <metadata_license>MIT</metadata_license>
  <project_license>MIT</project_license>
  <categories>
    <category>Game</category>
    <category>Utility</category>
  </categories>
</component>
XML

if [ ! -x "$TOOL" ]; then
    echo "↓ baixando appimagetool…"
    for i in 1 2 3; do
        if curl -fL --retry 3 -o "$TOOL" \
            "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage"; then
            chmod +x "$TOOL"
            break
        fi
        [ "$i" = "3" ] && { echo "✗ falha ao baixar appimagetool" >&2; exit 1; }
        sleep 2
    done
fi

mkdir -p "$ROOT/build"
# extract-and-run: funciona sem FUSE (CI/containers)
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$TOOL" "$APPDIR" "$OUT"

echo "✓ AppImage: $OUT ($(du -h "$OUT" | cut -f1))"
sha256sum "$OUT" > "$OUT.sha256"
echo "  sha256: $(cut -d' ' -f1 "$OUT.sha256")"
