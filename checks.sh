#!/usr/bin/env bash
# Checks the source tree, the winget manifest and what cleanshot.exe imports and is built with.
#   ./checks.sh                     Mac or Linux: builds a scan-only exe with cargo-xwin and
#                                   scans its imports and headers.
#   ./checks.sh --release-exe PATH  CI on windows-latest: scans the shipped exe.
# GATE_RANGE=A..B names the commits under test; the exe checks are skipped when they change no
# build input. The scan-only exe is never shipped and cannot render.
set -euo pipefail
shopt -s nullglob
cd "$(dirname "$0")"

llvm_bin=${LLVM_BIN:-/opt/homebrew/opt/llvm@22/bin}
lld_link=${LLD_LINK:-/opt/homebrew/opt/lld@22/bin/lld-link}
release_inputs=(Cargo.lock Cargo.toml .cargo rust-toolchain.toml crates checks.sh)
# Windows maps these from System32 before any search, so they may be imported statically:
# the KnownDLLs key on windows-latest, and ntdll, which every process has.
known_dlls=(
    advapi32.dll clbcatq.dll combase.dll comdlg32.dll coml2.dll gdi32.dll gdiplus.dll
    imagehlp.dll imm32.dll kernel32.dll msctf.dll msvcrt.dll normaliz.dll nsi.dll ntdll.dll
    ole32.dll oleaut32.dll psapi.dll rpcrt4.dll sechost.dll setupapi.dll shcore.dll shell32.dll
    shlwapi.dll user32.dll wldap32.dll
)
# Windows DLLs that build.rs delay-loads, so they load after main limits search to System32.
delay_loaded_dlls=(
    bcryptprimitives.dll comctl32.dll d2d1.dll d3d11.dll dcomp.dll dwmapi.dll dwrite.dll
    dxgi.dll icuuc.dll uiautomationcore.dll winmm.dll
)

case ${1:-} in
    --release-exe)
        exe=${2:?--release-exe needs the path of cleanshot.exe}
        kind="release exe"
        ;;
    "")
        exe=target/x86_64-pc-windows-msvc/scan-only/cleanshot.exe
        kind="scan-only exe"
        known_dlls+=(d3dcompiler_47.dll)
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
# Fails "$1<line>" for each line of $2, which the caller captured on its own line so that
# set -e stops on a check that could not run.
fail_each() {
    [[ -n $2 ]] || return 0
    while IFS= read -r line; do
        fail "$1$line"
    done <<<"$2"
}

roots=(crates/*/src/lib.rs crates/*/src/main.rs)
[[ ${#roots[@]} -gt 0 ]] || fail "no crate roots were found under crates/, so the unsafe scan did not run."
for root in "${roots[@]}"; do
    [[ $root == crates/cleanshot-win/* ]] && continue
    grep -qF '#![forbid(unsafe_code)]' "$root" || fail "$root lacks #![forbid(unsafe_code)]."
done
# git grep exits 1 for no match; anything higher is a scan that did not run.
unsafe_hits=$({ git grep -n -w unsafe -- 'crates/*.rs' ':!crates/cleanshot-win/' || [[ $? -eq 1 ]]; } | cut -d: -f1,2)
fail_each "unsafe outside crates/cleanshot-win at " "$unsafe_hits"

# Only @VERSION@ and @SHA256@ are left for a release to fill, so the rest of each winget file
# must already be what ships.
winget_line() {
    grep -qxF -- "$2" "winget/Cleanshot.Cleanshot.$1" || fail "winget/Cleanshot.Cleanshot.$1 lacks the line \"$2\"."
}
license=$(sed -n 's/^license = "\(.*\)"$/\1/p' Cargo.toml)
for file in yaml installer.yaml locale.en-US.yaml; do
    winget_line "$file" 'PackageIdentifier: Cleanshot.Cleanshot'
    winget_line "$file" 'PackageVersion: "@VERSION@"'
    winget_line "$file" 'ManifestVersion: 1.12.0'
done
winget_line installer.yaml 'InstallerType: portable'
winget_line installer.yaml '  - cleanshot'
# Windows 11 22H2.
winget_line installer.yaml 'MinimumOSVersion: 10.0.22621.0'
winget_line installer.yaml '  - Architecture: x64'
winget_line installer.yaml '    InstallerUrl: https://github.com/IconRunner05/cleanshot/releases/download/v@VERSION@/cleanshot-@VERSION@-x64.exe'
winget_line installer.yaml '    InstallerSha256: "@SHA256@"'
grep -qF 'cp "$RUNNER_TEMP/exe/cleanshot.exe" "dist/cleanshot-$VERSION-x64.exe"' .github/workflows/release.yml ||
    fail "release.yml no longer names the exe cleanshot-\$VERSION-x64.exe, the name the winget InstallerUrl downloads."
winget_line locale.en-US.yaml 'Publisher: Cleanshot'
winget_line locale.en-US.yaml "License: $license"

for crate in cleanshot-win cleanshot-editor; do
    features=$(cargo tree --locked -p cleanshot-app --target x86_64-pc-windows-msvc -e features -i "$crate")
    grep -qF "$crate feature \"field-test\"" <<<"$features" &&
        fail "cleanshot-app turns on $crate's field-test feature, so field-test code could ship in cleanshot.exe."
done

if [[ $kind == "scan-only exe" ]]; then
    if [[ -n ${GATE_RANGE:-} ]] && git diff --quiet "$GATE_RANGE" -- "${release_inputs[@]}"; then
        echo "Exe checks skipped: $GATE_RANGE changes no build input."
        exit "$failed"
    fi
    for tool in "$llvm_bin/llvm-readobj" "$llvm_bin/clang-cl" "$llvm_bin/llvm-lib" "$lld_link"; do
        [[ -x $tool ]] || { echo "FAIL $tool is missing; set LLVM_BIN and LLD_LINK, or install llvm@22 and lld@22 with Homebrew." >&2; exit 1; }
    done
    CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER="$lld_link" \
        CC_x86_64_pc_windows_msvc="$llvm_bin/clang-cl" \
        AR_x86_64_pc_windows_msvc="$llvm_bin/llvm-lib" \
        cargo xwin build --profile scan-only --locked --target x86_64-pc-windows-msvc -p cleanshot-app
fi

imports=$("$llvm_bin/llvm-readobj" --coff-imports "$exe")
# llvm-readobj lists each DLL in an "Import {" block, or "DelayImport {" when it is delay-loaded.
imported() {
    printf '%s\n' "$imports" |
        awk -v kind="$1" '/^[A-Za-z]+ [{]$/ { in_kind = ($1 == kind) } in_kind && /^  Name: / { print tolower($2) }' |
        sort -u
}
static_dlls=$(imported Import)
delayed_dlls=$(imported DelayImport)
dlls=$(printf '%s\n%s\n' "$static_dlls" "$delayed_dlls" | sed '/^$/d' | sort -u)
functions=$(printf '%s\n' "$imports" | sed -n 's/^ *Symbol: \([^ ]*\).*/\1/p' | sort -u)
[[ -n $dlls ]] || fail "llvm-readobj listed no DLLs for $exe, so the import scan did not run."

for dll in $dlls; do
    case $dll in
        ws2_32.dll | winhttp.dll | wininet.dll)
            fail "the $kind imports the network DLL $dll." ;;
        vcruntime* | msvcp* | ucrtbase* | api-ms-win-crt-*)
            fail "the $kind imports the C runtime DLL $dll; the CRT must be linked statically." ;;
        api-ms-win-*) ;;
        *)
            if grep -qx -F "$dll" <<<"$static_dlls"; then
                [[ " ${known_dlls[*]} " == *" $dll "* ]] ||
                    fail "the $kind imports $dll statically, and it is not a KnownDLL, so a copy beside the exe loads before main; delay-load it in build.rs."
            else
                [[ " ${delay_loaded_dlls[*]} " == *" $dll "* ]] ||
                    fail "the $kind delay-loads $dll, which is not on the delay-loaded DLL list in checks.sh."
            fi ;;
    esac
done

for function in $functions; do
    case $function in
        CreateProcess* | ShellExecute* | WinExec)
            fail "the $kind imports the process launcher $function." ;;
        SetCursorPos)
            fail "the $kind imports SetCursorPos, which only the UI Automation tests' drag uses; the product never moves the pointer." ;;
    esac
done
printf '%s\n' "$functions" | grep -qx SetDefaultDllDirectories ||
    fail "the $kind never calls SetDefaultDllDirectories, so DLLs can load from its own folder."

headers=$("$llvm_bin/llvm-readobj" --file-headers "$exe")
for mitigation in "ASLR:DYNAMIC_BASE" "High-entropy ASLR:HIGH_ENTROPY_VA" "DEP:NX_COMPAT" "Control Flow Guard:GUARD_CF"; do
    name=${mitigation%:*}
    flag=${mitigation#*:}
    grep -q "IMAGE_DLL_CHARACTERISTICS_$flag " <<<"$headers" ||
        fail "the $kind is built without $name (no $flag characteristic)."
done
debug_directory=$("$llvm_bin/llvm-readobj" --coff-debug-directory "$exe")
grep -q CET_COMPAT <<<"$debug_directory" ||
    fail "the $kind is built without CET shadow stack compatibility (no CET_COMPAT characteristic)."
grep -q "Subsystem: IMAGE_SUBSYSTEM_WINDOWS_GUI " <<<"$headers" ||
    fail "the $kind is not a Windows GUI exe, so a console window opens beside the tray."

manifest=$("$llvm_bin/llvm-readobj" --coff-resources "$exe" |
    awk '/^  Type: / { in_manifest = /\(ID 24\)/ } in_manifest && /\|.*\|$/ { sub(/^[^|]*\|/, ""); sub(/\|$/, ""); printf "%s", $0 }')
[[ $manifest == *">PerMonitorV2</dpiAwareness>"* ]] ||
    fail "the $kind's embedded manifest does not declare PerMonitorV2 DPI awareness."

# Each icon group as "id: widths", read from its RT_GROUP_ICON directory (a width byte
# of 0 means 256).
icon_groups=$("$llvm_bin/llvm-readobj" --coff-resources "$exe" | awk '
    function byte(at) { return (index("0123456789ABCDEF", substr(hex[group], at, 1)) - 1) * 16 + index("0123456789ABCDEF", substr(hex[group], at + 1, 1)) - 1 }
    /^  Type: / { in_groups = /\(ID 14\)/ }
    in_groups && /^    Name: \(ID / { group = $3; sub(/\)/, "", group); hex[group] = "" }
    in_groups && /^            [0-9A-F]+: / { for (i = 2; i <= 5 && $i !~ /^\|/; i++) hex[group] = hex[group] $i }
    END {
        for (group in hex) {
            line = group ":"
            for (i = 0; i < byte(9); i++) { width = byte(13 + 28 * i); line = line " " (width == 0 ? 256 : width) }
            print line
        }
    }' | sort)
[[ $icon_groups == '1: 16 20 24 32 40 48 64 96 256' ]] ||
    fail "the $kind's icon groups are \"${icon_groups//$'\n'/; }\", not the app icon alone as group 1 at 16 to 256 px."
echo "Icon groups (id: sizes in px): ${icon_groups//$'\n'/; }."

if [[ $kind == "release exe" ]]; then
    # Windows keeps many strings as UTF-16, which llvm-strings does not read, so the exe is
    # also decoded as UTF-16LE at both byte alignments.
    strings=$(
        "$llvm_bin/llvm-strings" "$exe" &&
        # iconv exits 1 on the half character an odd length leaves at the end; the rest is read.
        { { iconv -f UTF-16LE -t UTF-8 -c "$exe" 2>/dev/null || [[ $? -eq 1 ]]; } &&
          { tail -c +2 "$exe" | iconv -f UTF-16LE -t UTF-8 -c 2>/dev/null || [[ $? -eq 1 ]]; }; } |
            LC_ALL=C tr -c '[:print:]' '\n' | LC_ALL=C grep -a -E '.{4}'
    )
    host_paths=$(printf '%s\n' "$strings" | grep -a -i -F -e /Users/ -e /home/ -e '\Users\' -e '\.cargo\' -e ':\a\' || [[ $? -eq 1 ]])
    [[ -z $host_paths ]] ||
        fail "the release exe holds $(printf '%s\n' "$host_paths" | wc -l | tr -d ' ') build-host paths, such as $(printf '%s\n' "$host_paths" | head -1)."
fi

size=$(wc -c <"$exe" | tr -d ' ')
echo "The $kind: $size bytes, $(printf '%s\n' "$dlls" | wc -l | tr -d ' ') DLLs, $(printf '%s\n' "$functions" | wc -l | tr -d ' ') imported functions."

exit "$failed"
