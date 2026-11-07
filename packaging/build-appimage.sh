#!/usr/bin/env bash
# Builds dist/Ratewise-x86_64.AppImage (spec §6).
#
#   packaging/build-appimage.sh
#
# Needs cargo-zigbuild, curl, dpkg-deb and python3. Pinned tools and libraries are
# downloaded once into RATEWISE_TOOLS (default ~/.cache/ratewise-tools) and checked
# against the hashes in versions.env.
set -euo pipefail
cd "$(dirname "$0")/.."
. packaging/versions.env
tools=${RATEWISE_TOOLS:-$HOME/.cache/ratewise-tools}
mkdir -p "$tools" dist

fetch() { # url file sha256
    local path=$tools/$2
    if [ ! -f "$path" ]; then
        echo "Downloading $2…"
        curl -fsSL -o "$path.part" "$1"
        mv "$path.part" "$path"
    fi
    echo "$3  $path" | sha256sum -c --quiet -
}
fetch "$APPIMAGETOOL_URL" "$APPIMAGETOOL_FILE" "$APPIMAGETOOL_SHA256"
fetch "$RUNTIME_URL" "$RUNTIME_FILE" "$RUNTIME_SHA256"
chmod +x "$tools/$APPIMAGETOOL_FILE"

echo "Building the release binary (glibc 2.28)…"
triple=x86_64-unknown-linux-gnu
cargo zigbuild --release --locked --target "$triple.2.28"
target_dir=$(cargo metadata --format-version 1 --no-deps |
    grep -o '"target_directory":"[^"]*"' | cut -d'"' -f4)
bin=$target_dir/$triple/release/ratewise
newest=$(objdump -T "$bin" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1)
if [ "$(printf '%s\nGLIBC_2.28\n' "$newest" | sort -V | tail -1)" != GLIBC_2.28 ]; then
    echo "The binary needs $newest, newer than the 2.28 floor." >&2
    exit 1
fi

libroot=$(packaging/fetch-libs.sh)

app=$target_dir/appdir
rm -rf "$app"
mkdir -p "$app/usr/bin" "$app/usr/lib/ratewise" "$app/usr/share/applications" \
    "$app/usr/share/metainfo" "$app/usr/share/icons/hicolor/256x256/apps" \
    "$app/usr/share/icons/hicolor/scalable/apps" "$app/usr/share/licenses/ratewise"

install -m 755 "$bin" "$app/usr/bin/ratewise"
install -m 755 packaging/AppRun "$app/AppRun"
id=io.github.sof006.ratewise
install -m 644 packaging/$id.desktop "$app/$id.desktop"
install -m 644 packaging/$id.desktop "$app/usr/share/applications/$id.desktop"
install -m 644 packaging/$id.metainfo.xml "$app/usr/share/metainfo/$id.metainfo.xml"
install -m 644 packaging/ratewise.png "$app/ratewise.png"
install -m 644 packaging/ratewise.png "$app/usr/share/icons/hicolor/256x256/apps/ratewise.png"
install -m 644 packaging/ratewise.svg "$app/usr/share/icons/hicolor/scalable/apps/ratewise.svg"
ln -s ratewise.png "$app/.DirIcon"

lic=$app/usr/share/licenses/ratewise
install -m 644 LICENSE "$lic/LICENSE"
install -m 644 assets/fonts/Inter-LICENSE.txt "$lic/Inter-LICENSE.txt"
for doc in "$libroot"/usr/share/doc/*/copyright; do
    pkg=$(basename "$(dirname "$doc")")
    install -m 644 "$doc" "$lic/$pkg.copyright"
done

grep -v '^#' packaging/bundled-libs.txt | grep . | while read -r soname; do
    src=
    for d in lib/x86_64-linux-gnu usr/lib/x86_64-linux-gnu; do
        [ -e "$libroot/$d/$soname" ] && src=$libroot/$d/$soname
    done
    if [ -z "$src" ]; then
        echo "$soname isn't in the Debian packages; add its package to fetch-libs.sh." >&2
        exit 1
    fi
    install -m 644 "$(readlink -f "$src")" "$app/usr/lib/ratewise/$soname"
done

name=Ratewise-x86_64.AppImage
# Pack inside the target dir: appimagetool marks the file executable, which fails on
# mounts without Unix permissions (such as a Windows drive under WSL).
packed=$target_dir/$name
rm -f "$packed"
echo "Packing $name…"
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$tools/$APPIMAGETOOL_FILE" \
    --runtime-file "$tools/$RUNTIME_FILE" "$app" "$packed" >/dev/null
cp "$packed" "dist/$name"
chmod +x "dist/$name" 2>/dev/null || true
(cd dist && sha256sum "$name" > "$name.sha256")
ls -l "dist/$name"
cat "dist/$name.sha256"
