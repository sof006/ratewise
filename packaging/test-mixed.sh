#!/usr/bin/env bash
# A host that lacks some of the bundled libraries gets a mix of bundled and host
# copies. For each bundled library, this pretends only that one is missing and
# checks the window still renders (`--self-test` under Xvfb). Mixing libxkbcommon
# versions once crashed on GitHub's runners this way.
#
#   packaging/test-mixed.sh dist/Ratewise-x86_64.AppImage
set -uo pipefail
image=$(readlink -f "${1:?usage: test-mixed.sh <AppImage>}")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
(cd "$work" && "$image" --appimage-extract >/dev/null)
app=$work/squashfs-root
failed=()
for f in "$app"/usr/lib/ratewise/*.so*; do
    n=$(basename "$f")
    if out=$(RATEWISE_LIBS_MISSING=$n RATEWISE_RENDERER=software timeout 60 \
        xvfb-run -a "$app/AppRun" --self-test 2>&1); then
        echo "ok      $n"
    else
        echo "FAILED  $n: $(echo "$out" | tail -1)"
        failed+=("$n")
    fi
done
if [ ${#failed[@]} -gt 0 ]; then
    echo "Failed with: ${failed[*]}"
    exit 1
fi
echo "Every single-library mix renders."
