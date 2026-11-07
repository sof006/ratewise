#!/usr/bin/env bash
# Fetches the libraries the window needs from Debian 10 (glibc 2.28) and extracts
# them to $RATEWISE_TOOLS/buster/root. Prints that directory's lib folder.
#
# Every .deb is checked against the SHA-256 in Debian's package index, and the
# index against the Release file's hash, pinned in versions.env.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
. "$here/versions.env"
tools=${RATEWISE_TOOLS:-$HOME/.cache/ratewise-tools}
dir=$tools/$DEBIAN_SUITE
mkdir -p "$dir/debs"

# The window's runtime libraries (see bundled-libs.txt) and what they pull in.
packages=(
    libfontconfig1 libfreetype6 libexpat1 libpng16-16 zlib1g
    libx11-6 libx11-xcb1 libxcb1 libxcb-xkb1 libxau6 libxdmcp6 libbsd0
    libxcursor1 libxrender1 libxfixes3 libxi6 libxext6
    libxkbcommon0 libxkbcommon-x11-0
    libwayland-client0 libwayland-cursor0 libwayland-egl1 libffi6 libuuid1
)

index=$dir/Packages.gz
if [ ! -f "$index" ]; then
    echo "Downloading the Debian $DEBIAN_SUITE package index…" >&2
    curl -fsSL -o "$index.part" "$DEBIAN_MIRROR/dists/$DEBIAN_SUITE/main/binary-amd64/Packages.gz"
    mv "$index.part" "$index"
fi
echo "$DEBIAN_PACKAGES_SHA256  $index" | sha256sum -c --quiet - >&2

root=$dir/root
rm -rf "$root"
mkdir -p "$root"
for p in "${packages[@]}"; do
    # Filename and SHA256 of the package's stanza.
    read -r file sha < <(zcat "$index" | awk -v p="$p" '
        $0 == "Package: " p { on = 1; next }
        on && /^Filename: / { f = $2 }
        on && /^SHA256: / { s = $2 }
        on && /^$/ { print f, s; exit }')
    if [ -z "${file:-}" ]; then
        echo "Package $p isn't in the $DEBIAN_SUITE index." >&2
        exit 1
    fi
    deb=$dir/debs/$(basename "$file")
    if [ ! -f "$deb" ]; then
        echo "Downloading $(basename "$file")…" >&2
        curl -fsSL -o "$deb.part" "$DEBIAN_MIRROR/$file"
        mv "$deb.part" "$deb"
    fi
    echo "$sha  $deb" | sha256sum -c --quiet - >&2
    dpkg-deb -x "$deb" "$root"
done
echo "$root"
