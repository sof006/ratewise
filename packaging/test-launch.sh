#!/usr/bin/env bash
# The launch matrix (spec §6.4): in minimal containers with no audio packages, the
# AppImage must render a frame (`--self-test`) and `doctor` must exit 0. The extra
# "wayland" run uses a headless Weston to exercise the Wayland path. Needs Docker
# or Podman.
#
#   packaging/test-launch.sh dist/Ratewise-x86_64.AppImage            # everything
#   packaging/test-launch.sh dist/Ratewise-x86_64.AppImage debian:11  # some of it
set -uo pipefail
image=$(readlink -f "${1:?usage: test-launch.sh <AppImage> [image|wayland ...]}")
shift
engine=${CONTAINER_ENGINE:-$(command -v docker || command -v podman || true)}
if [ -z "$engine" ]; then
    echo "Neither docker nor podman was found." >&2
    exit 2
fi

apt='apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends xvfb xauth >/dev/null'
declare -A setup=(
    [ubuntu:20.04]=$apt
    [ubuntu:22.04]=$apt
    [ubuntu:24.04]=$apt
    [debian:11]=$apt
    [debian:12]=$apt
    [fedora:latest]='dnf install -y -q xorg-x11-server-Xvfb xvfb-run xorg-x11-xauth >/dev/null'
    [opensuse/tumbleweed]='zypper -nq in xvfb-run gawk >/dev/null'
    [archlinux:latest]='pacman -Sy --noconfirm --quiet xorg-server-xvfb xorg-xauth >/dev/null'
)
all=(ubuntu:20.04 ubuntu:22.04 ubuntu:24.04 debian:11 debian:12 fedora:latest opensuse/tumbleweed archlinux:latest wayland)
if [ $# -gt 0 ]; then targets=("$@"); else targets=("${all[@]}"); fi

# 24-bit like a real desktop: openSUSE's xvfb-run defaults to 8-bit, where neither
# OpenGL nor the software renderer can draw.
x11='set -e
A="/dist/app --appimage-extract-and-run"
xvfb-run -a -s "-screen 0 1280x1024x24" $A --self-test
$A doctor >/dev/null
echo "doctor exit 0"'

wayland='set -e
pacman -Sy --noconfirm --quiet weston >/dev/null
export XDG_RUNTIME_DIR=/tmp/xdg; mkdir -p -m 700 $XDG_RUNTIME_DIR
weston --backend=headless --socket=wayland-1 >/tmp/weston.log 2>&1 &
for i in $(seq 50); do [ -S $XDG_RUNTIME_DIR/wayland-1 ] && break; sleep 0.2; done
unset DISPLAY; export WAYLAND_DISPLAY=wayland-1
/dist/app --appimage-extract-and-run --self-test'

failed=()
for t in "${targets[@]}"; do
    if [ "$t" = wayland ]; then
        img=archlinux:latest
        script=$wayland
        echo "=== $img (Wayland, headless Weston)"
    elif [ -n "${setup[$t]:-}" ]; then
        img=$t
        script="${setup[$t]} && $x11"
        echo "=== $img"
    else
        echo "Unknown target $t. Known: ${all[*]}" >&2
        exit 2
    fi
    if "$engine" run --rm -v "$image:/dist/app:ro" "$img" sh -c "$script"; then
        echo "--- $t passed"
    else
        echo "--- $t FAILED"
        failed+=("$t")
    fi
done

if [ ${#failed[@]} -gt 0 ]; then
    echo "Failed: ${failed[*]}"
    exit 1
fi
echo "All launches passed."
