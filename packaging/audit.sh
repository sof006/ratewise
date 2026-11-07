#!/usr/bin/env bash
# Checks an AppImage against spec §6 (run in CI):
#   - every ELF inside needs nothing newer than glibc 2.28;
#   - with the bundled libraries on the path (as on a host that has none of them),
#     every dependency resolves, either to a bundled copy or to a host library on the
#     allowlist (glibc family, libgcc_s, GPU driver stack);
#   - the runtime is the static type2 one, so libfuse2 isn't needed.
#
#   packaging/audit.sh dist/Ratewise-x86_64.AppImage
set -euo pipefail
image=$(readlink -f "${1:?usage: audit.sh <AppImage>}")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
(cd "$work" && "$image" --appimage-extract >/dev/null)
root=$work/squashfs-root
bundled=$root/usr/lib/ratewise
fail=0

allowed='^(libc|libm|libdl|libpthread|librt|libresolv|libutil|ld-linux[^ ]*|linux-vdso|libgcc_s|libGL|libEGL|libGLX|libGLdispatch|libvulkan|libdrm|libgbm)\.so'

# The runtime: a static ELF at the start of the file.
if file -L "$image" | grep -q 'dynamically linked'; then
    echo "FAIL runtime is dynamically linked (the type2 static runtime is required)"
    fail=1
fi

while IFS= read -r -d '' f; do
    file -L "$f" | grep -q ELF || continue
    rel=${f#"$root"/}
    newest=$(objdump -T "$f" 2>/dev/null | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1)
    if [ -n "$newest" ] && [ "$(printf '%s\nGLIBC_2.28\n' "$newest" | sort -V | tail -1)" != GLIBC_2.28 ]; then
        echo "FAIL $rel needs $newest"
        fail=1
    fi
    while read -r line; do
        case $line in
            *"not found"*)
                echo "FAIL $rel: ${line%% =>*} not found"
                fail=1
                ;;
            *"=> $bundled/"*) ;;
            *"=>"*)
                lib=${line%% =>*}
                if ! echo "$lib" | grep -Eq "$allowed"; then
                    echo "FAIL $rel: $lib comes from the host (${line#*=> })"
                    fail=1
                fi
                ;;
        esac
    done < <(LD_LIBRARY_PATH=$bundled ldd "$f" 2>/dev/null || true)
done < <(find "$root" -type f -print0)

# Everything the binary dlopens must be bundled or allowed.
for name in $(strings "$root/usr/bin/ratewise" | grep -oE 'lib[A-Za-z0-9_+-]+\.so\.[0-9]+' | sort -u); do
    echo "$name" | grep -Eq "$allowed" && continue
    if [ ! -e "$bundled/$name" ]; then
        echo "FAIL usr/bin/ratewise loads $name, which isn't bundled"
        fail=1
    fi
done

if [ "$fail" = 0 ]; then
    echo "audit passed: $(find "$bundled" -name '*.so*' | wc -l) bundled libraries, glibc floor 2.28"
fi
exit $fail
