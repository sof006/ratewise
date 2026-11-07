#!/usr/bin/env bash
# The launch matrix (spec §6.4): in minimal containers with no audio packages, the
# AppImage must render a frame (`--self-test`) and `doctor` must exit 0. One extra
# run uses a headless Weston to exercise the Wayland path. Needs Docker or Podman.
#
#   packaging/test-launch.sh dist/Ratewise-x86_64.AppImage
set -uo pipefail
image=$(readlink -f "${1:?usage: test-launch.sh <AppImage>}")
engine=${CONTAINER_ENGINE:-$(command -v docker || command -v podman || true)}
if [ -z "$engine" ]; then
    echo "Neither docker nor podman was found." >&2
    exit 2
fi

run='set -e
A="/dist/app --appimage-extract-and-run"
xvfb-run -a $A --self-test
$A doctor >/dev/null
echo "doctor exit 0"'

declare -A setup=(
    [ubuntu:20.04]='apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends xvfb xauth >/dev/null'
    [ubuntu:22.04]='apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends xvfb xauth >/dev/null'
    [ubuntu:24.04]='apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends xvfb xauth >/dev/null'
    [debian:11]='apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends xvfb xauth >/dev/null'
    [debian:12]='apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends xvfb xauth >/dev/null'
    [fedora:latest]='dnf install -y -q xorg-x11-server-Xvfb xvfb-run xorg-x11-xauth >/dev/null'
    [opensuse/tumbleweed]='zypper -nq in xvfb-run >/dev/null'
    [archlinux:latest]='pacman -Sy --noconfirm --quiet xorg-server-xvfb xorg-xauth >/dev/null'
)
order=(ubuntu:20.04 ubuntu:22.04 ubuntu:24.04 debian:11 debian:12 fedora:latest opensuse/tumbleweed archlinux:latest)

failed=()
for img in "${order[@]}"; do
    echo "=== $img"
    if "$engine" run --rm -v "$image:/dist/app:ro" "$img" sh -c "${setup[$img]} && $run"; then
        echo "--- $img passed"
    else
        echo "--- $img FAILED"
        failed+=("$img")
    fi
done

echo "=== archlinux:latest (Wayland, headless Weston)"
wayland='pacman -Sy --noconfirm --quiet weston >/dev/null
export XDG_RUNTIME_DIR=/tmp/xdg; mkdir -p -m 700 $XDG_RUNTIME_DIR
weston --backend=headless --socket=wayland-1 >/tmp/weston.log 2>&1 &
for i in $(seq 50); do [ -S $XDG_RUNTIME_DIR/wayland-1 ] && break; sleep 0.2; done
unset DISPLAY; export WAYLAND_DISPLAY=wayland-1
/dist/app --appimage-extract-and-run --self-test'
if "$engine" run --rm -v "$image:/dist/app:ro" archlinux:latest sh -c "$wayland"; then
    echo "--- wayland passed"
else
    echo "--- wayland FAILED"
    failed+=("archlinux:latest (wayland)")
fi

if [ ${#failed[@]} -gt 0 ]; then
    echo "Failed: ${failed[*]}"
    exit 1
fi
echo "All launches passed."
