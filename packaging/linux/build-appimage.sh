#!/usr/bin/env bash
# Costruisce target/appimage/grabbit-linux-<arch>.AppImage
# Uso:  packaging/linux/build-appimage.sh
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/../.."
ARCH="${ARCH:-$(uname -m)}"            # x86_64 | aarch64
OUT="target/appimage"
APPDIR="$OUT/Grabbit.AppDir"

cargo build --release

rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" \
         "$APPDIR/usr/share/applications" \
         "$APPDIR/usr/share/metainfo" \
         "$APPDIR/usr/share/icons/hicolor/512x512/apps"

cp target/release/grabbit                "$APPDIR/usr/bin/grabbit"
# nome del file .desktop = identificativo AppStream (forma «io.github.…»): appimagetool
# cerca la descrizione con lo stesso nome e appstreamcli vuole che coincidano
ID="io.github.paselsoft.grabbit"
cp "packaging/linux/$ID.desktop"         "$APPDIR/$ID.desktop"
cp "packaging/linux/$ID.desktop"         "$APPDIR/usr/share/applications/$ID.desktop"
cp "packaging/linux/$ID.appdata.xml"     "$APPDIR/usr/share/metainfo/$ID.appdata.xml"
cp assets/icon.png                       "$APPDIR/grabbit.png"
cp assets/icon.png                       "$APPDIR/usr/share/icons/hicolor/512x512/apps/grabbit.png"
ln -sf usr/bin/grabbit                   "$APPDIR/AppRun"

TOOL="$OUT/appimagetool-$ARCH.AppImage"
if [ ! -x "$TOOL" ]; then
  mkdir -p "$OUT"
  curl -fsSL -o "$TOOL" \
    "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$ARCH.AppImage"
  chmod +x "$TOOL"
fi

# --appimage-extract-and-run: funziona anche dove FUSE non è disponibile (CI, container)
ARCH="$ARCH" "$TOOL" --appimage-extract-and-run "$APPDIR" "$OUT/grabbit-linux-$ARCH.AppImage"
echo "Creato: $OUT/grabbit-linux-$ARCH.AppImage"
