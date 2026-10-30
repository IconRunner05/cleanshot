#!/usr/bin/env bash
# CHARTER 7 gates 3-5 on cleanshot.exe.
#   ./charter-gates.sh                     Mac or Linux fold bar: builds a scan-only exe with
#                                          cargo-xwin and scans its imports and headers.
#   ./charter-gates.sh --release-exe PATH  CI on windows-latest: scans the shipped exe, size too.
# D-21: the scan-only exe is never shipped and cannot render.
set -euo pipefail
shopt -s nullglob
cd "$(dirname "$0")"

llvm_bin=${LLVM_BIN:-/opt/homebrew/opt/llvm@22/bin}
lld_link=${LLD_LINK:-/opt/homebrew/opt/lld@22/bin/lld-link}
size_budget_bytes=15000000
release_inputs=(Cargo.lock Cargo.toml .cargo rust-toolchain.toml crates charter-gates.sh)
system_dlls=(
    advapi32.dll bcryptprimitives.dll combase.dll comctl32.dll d3d11.dll dcomp.dll dwmapi.dll
    dwrite.dll dxgi.dll gdi32.dll icuuc.dll imm32.dll kernel32.dll ntdll.dll ole32.dll
    oleaut32.dll shell32.dll uiautomationcore.dll user32.dll winmm.dll
)

case ${1:-} in
    --release-exe)
        exe=${2:?--release-exe needs the path of cleanshot.exe}
        kind="release exe"
        ;;
    "")
        exe=target/x86_64-pc-windows-msvc/scan-only/cleanshot.exe
        kind="scan-only exe"
        system_dlls+=(d3dcompiler_47.dll)
        ;;
    *)
        echo "usage: $0 [--release-exe PATH]" >&2
        exit 2
        ;;
esac

failed=0
fail() {
    echo "FAIL $1" >&2
    failed=1
}

roots=(crates/*/src/lib.rs crates/*/src/main.rs)
[[ ${#roots[@]} -gt 0 ]] || fail "C-16: no crate roots were found under crates/, so the unsafe scan did not run."
for root in "${roots[@]}"; do
    [[ $root == crates/cleanshot-win/* ]] && continue
    grep -qF '#![forbid(unsafe_code)]' "$root" || fail "C-16: $root lacks #![forbid(unsafe_code)]."
done
while IFS= read -r hit; do
    fail "C-16: unsafe outside crates/cleanshot-win at $hit."
done < <(git grep -n -w unsafe -- 'crates/*.rs' ':!crates/cleanshot-win/' | cut -d: -f1,2)

if [[ $kind == "scan-only exe" ]]; then
    if [[ -n ${LANES_FOLD_RANGE:-} ]] && git diff --quiet "$LANES_FOLD_RANGE" -- "${release_inputs[@]}"; then
        echo "Exe gates skipped: $LANES_FOLD_RANGE changes no build input."
        exit "$failed"
    fi
    for tool in "$llvm_bin/llvm-readobj" "$llvm_bin/clang-cl" "$llvm_bin/llvm-lib" "$lld_link"; do
        [[ -x $tool ]] || { echo "FAIL $tool is missing; set LLVM_BIN and LLD_LINK, or install llvm@22 and lld@22 with Homebrew." >&2; exit 1; }
    done
    lanes build -- env \
        CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER="$lld_link" \
        CC_x86_64_pc_windows_msvc="$llvm_bin/clang-cl" \
        AR_x86_64_pc_windows_msvc="$llvm_bin/llvm-lib" \
        cargo xwin build --profile scan-only --locked --target x86_64-pc-windows-msvc -p cleanshot-app
fi

imports=$("$llvm_bin/llvm-readobj" --coff-imports "$exe")
dlls=$(printf '%s\n' "$imports" | sed -n 's/^  Name: //p' | tr '[:upper:]' '[:lower:]' | sort -u)
functions=$(printf '%s\n' "$imports" | sed -n 's/^  Symbol: \([^ ]*\).*/\1/p' | sort -u)
[[ -n $dlls ]] || fail "C-02: llvm-readobj listed no DLLs for $exe, so the import scan did not run."

for dll in $dlls; do
    case $dll in
        ws2_32.dll | winhttp.dll | wininet.dll)
            fail "C-10: the $kind imports the network DLL $dll." ;;
        vcruntime* | msvcp* | ucrtbase* | api-ms-win-crt-*)
            fail "C-02: the $kind imports the C runtime DLL $dll; the CRT must be linked statically." ;;
        api-ms-win-*) ;;
        *)
            [[ " ${system_dlls[*]} " == *" $dll "* ]] ||
                fail "C-02: the $kind imports $dll, which is not on the system DLL list in charter-gates.sh." ;;
    esac
done

for function in $functions; do
    case $function in
        CreateProcess* | ShellExecute* | WinExec)
            fail "C-11: the $kind imports the process launcher $function." ;;
    esac
done

headers=$("$llvm_bin/llvm-readobj" --file-headers "$exe")
for mitigation in "ASLR:DYNAMIC_BASE" "DEP:NX_COMPAT" "Control Flow Guard:GUARD_CF"; do
    name=${mitigation%:*}
    flag=${mitigation#*:}
    grep -q "IMAGE_DLL_CHARACTERISTICS_$flag " <<<"$headers" ||
        fail "C-17: the $kind is built without $name (no $flag characteristic)."
done

if [[ $kind == "release exe" ]]; then
    strings=$("$llvm_bin/llvm-strings" "$exe")
    host_paths=$(printf '%s\n' "$strings" | grep -i -F -e /Users/ -e /home/ -e 'C:\Users\' -e '\.cargo\' || [[ $? -eq 1 ]])
    [[ -z $host_paths ]] ||
        fail "C-20: the release exe holds $(printf '%s\n' "$host_paths" | wc -l | tr -d ' ') build-host paths, such as $(printf '%s\n' "$host_paths" | head -1)."
fi

size=$(wc -c <"$exe" | tr -d ' ')
echo "The $kind: $size bytes, $(printf '%s\n' "$dlls" | wc -l | tr -d ' ') DLLs, $(printf '%s\n' "$functions" | wc -l | tr -d ' ') imported functions."
if [[ $kind == "release exe" && $size -ge $size_budget_bytes ]]; then
    fail "P-06: the release exe is $size bytes; the budget is under $size_budget_bytes bytes (15 MB)."
fi

exit "$failed"
