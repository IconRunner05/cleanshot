#!/usr/bin/env bash
# CHARTER 7 gates 3 and 4 on cleanshot.exe, the CHARTER 9 winget manifest, and the CHARTER 11
# Decisions log (docs/decisions.md).
#   ./charter-gates.sh                     Mac or Linux fold bar: builds a scan-only exe with
#                                          cargo-xwin and scans its imports and headers.
#   ./charter-gates.sh --release-exe PATH  CI on windows-latest: scans the shipped exe.
# D-21: the scan-only exe is never shipped and cannot render.
set -euo pipefail
shopt -s nullglob
cd "$(dirname "$0")"

llvm_bin=${LLVM_BIN:-/opt/homebrew/opt/llvm@22/bin}
lld_link=${LLD_LINK:-/opt/homebrew/opt/lld@22/bin/lld-link}
release_inputs=(Cargo.lock Cargo.toml .cargo rust-toolchain.toml crates charter-gates.sh)
# C-12: Windows maps these from System32 before any search, so they may be imported statically:
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

roots=(crates/*/src/lib.rs crates/*/src/main.rs)
[[ ${#roots[@]} -gt 0 ]] || fail "C-16: no crate roots were found under crates/, so the unsafe scan did not run."
for root in "${roots[@]}"; do
    [[ $root == crates/cleanshot-win/* ]] && continue
    grep -qF '#![forbid(unsafe_code)]' "$root" || fail "C-16: $root lacks #![forbid(unsafe_code)]."
done
while IFS= read -r hit; do
    fail "C-16: unsafe outside crates/cleanshot-win at $hit."
done < <(git grep -n -w unsafe -- 'crates/*.rs' ':!crates/cleanshot-win/' | cut -d: -f1,2)

while IFS= read -r problem; do
    fail "CHARTER 11: $problem"
done < <(awk '
    /^\| D-[0-9]+ \|/ {
        split($0, cell, " [|] ")
        id = substr(cell[1], 5) + 0
        rows++
        if (id in line_of) printf "D-%d is the id of the rows at lines %d and %d.\n", id, line_of[id], NR
        status_allowed = "^(assumed|decided [(]owner, [0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9][)])$"
        if (cell[4] !~ status_allowed) printf "D-%d at line %d has the Status \"%s\", not assumed or decided (owner, YYYY-MM-DD).\n", id, NR, cell[4]
        line_of[id] = NR
    }
    END { if (!rows) print "no Decisions log rows were found in docs/decisions.md, so the log was not checked." }
' docs/decisions.md)

# CHARTER 9, D-98: only @VERSION@ and @SHA256@ are left for a release to fill, so the rest of each winget
# file must already hold what the charter and Cargo.toml say.
winget_line() {
    grep -qxF -- "$2" "winget/Cleanshot.Cleanshot.$1" || fail "CHARTER 9: winget/Cleanshot.Cleanshot.$1 lacks the line \"$2\"."
}
license=$(sed -n 's/^license = "\(.*\)"$/\1/p' Cargo.toml)
for file in yaml installer.yaml locale.en-US.yaml; do
    winget_line "$file" 'PackageIdentifier: Cleanshot.Cleanshot'
    winget_line "$file" 'PackageVersion: "@VERSION@"'
    winget_line "$file" 'ManifestVersion: 1.12.0'
done
winget_line installer.yaml 'InstallerType: portable'
winget_line installer.yaml '  - cleanshot'
# C-01: Windows 11 22H2.
winget_line installer.yaml 'MinimumOSVersion: 10.0.22621.0'
winget_line installer.yaml '  - Architecture: x64'
winget_line installer.yaml '    InstallerUrl: https://github.com/IconRunner05/cleanshot/releases/download/v@VERSION@/cleanshot-@VERSION@-x64.exe'
winget_line installer.yaml '    InstallerSha256: "@SHA256@"'
grep -qF 'cp "$RUNNER_TEMP/exe/cleanshot.exe" "dist/cleanshot-$VERSION-x64.exe"' .github/workflows/release.yml ||
    fail "CHARTER 9: release.yml no longer names the exe cleanshot-\$VERSION-x64.exe, the name the winget InstallerUrl downloads."
winget_line locale.en-US.yaml 'Publisher: Cleanshot'
winget_line locale.en-US.yaml "License: $license"

# A commit naming a charter id that changes product code says so in CHANGELOG.md, or says why
# not in a "No CHANGELOG: <why>" line of its message. A warning only. CI's shallow clone has no
# parent to compare.
changelog_range=${LANES_FOLD_RANGE:-HEAD~1..HEAD}
git rev-parse -q --verify "${changelog_range%%..*}^{commit}" >/dev/null || changelog_range=
while IFS= read -r commit; do
    subject=$(git log -1 --format=%s "$commit")
    [[ $subject =~ (^|[^A-Za-z])[FCD]-[0-9]+ ]] || continue
    [[ $'\n'$(git log -1 --format=%B "$commit") =~ $'\n'"No CHANGELOG: "[^[:space:]] ]] && continue
    git diff --quiet "$commit^" "$commit" -- ':(glob)crates/*/src/**' && continue
    git diff --quiet "$commit^" "$commit" -- CHANGELOG.md || continue
    echo "WARN CHANGELOG.md: ${commit:0:9} \"$subject\" changes crates/*/src with no CHANGELOG.md line and no \"No CHANGELOG: <why>\" line." >&2
done < <([[ -z $changelog_range ]] || git rev-list --no-merges "$changelog_range")

cargo tree --locked -p cleanshot-app --target x86_64-pc-windows-msvc -e features -i cleanshot-win | grep -qF 'cleanshot-win feature "field-test"' &&
    fail "CHARTER 5: cleanshot-app turns on cleanshot-win's field-test feature, so field-test code could ship in cleanshot.exe."

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
[[ -n $dlls ]] || fail "C-02: llvm-readobj listed no DLLs for $exe, so the import scan did not run."

for dll in $dlls; do
    case $dll in
        ws2_32.dll | winhttp.dll | wininet.dll)
            fail "C-10: the $kind imports the network DLL $dll." ;;
        vcruntime* | msvcp* | ucrtbase* | api-ms-win-crt-*)
            fail "C-02: the $kind imports the C runtime DLL $dll; the CRT must be linked statically." ;;
        api-ms-win-*) ;;
        *)
            if grep -qx -F "$dll" <<<"$static_dlls"; then
                [[ " ${known_dlls[*]} " == *" $dll "* ]] ||
                    fail "C-12: the $kind imports $dll statically, and it is not a KnownDLL, so a copy beside the exe loads before main; delay-load it in build.rs."
            else
                [[ " ${delay_loaded_dlls[*]} " == *" $dll "* ]] ||
                    fail "C-02: the $kind delay-loads $dll, which is not on the delay-loaded DLL list in charter-gates.sh."
            fi ;;
    esac
done

for function in $functions; do
    case $function in
        CreateProcess* | ShellExecute* | WinExec)
            fail "C-11: the $kind imports the process launcher $function." ;;
    esac
done
printf '%s\n' "$functions" | grep -qx SetDefaultDllDirectories ||
    fail "C-12: the $kind never calls SetDefaultDllDirectories, so DLLs can load from its own folder."

headers=$("$llvm_bin/llvm-readobj" --file-headers "$exe")
for mitigation in "ASLR:DYNAMIC_BASE" "DEP:NX_COMPAT" "Control Flow Guard:GUARD_CF"; do
    name=${mitigation%:*}
    flag=${mitigation#*:}
    grep -q "IMAGE_DLL_CHARACTERISTICS_$flag " <<<"$headers" ||
        fail "C-17: the $kind is built without $name (no $flag characteristic)."
done
grep -q "Subsystem: IMAGE_SUBSYSTEM_WINDOWS_GUI " <<<"$headers" ||
    fail "CHARTER 7 gate 3: the $kind is not a Windows GUI exe, so a console window opens beside the tray."

manifest=$("$llvm_bin/llvm-readobj" --coff-resources "$exe" |
    awk '/^  Type: / { in_manifest = /\(ID 24\)/ } in_manifest && /\|.*\|$/ { sub(/^[^|]*\|/, ""); sub(/\|$/, ""); printf "%s", $0 }')
[[ $manifest == *">PerMonitorV2</dpiAwareness>"* ]] ||
    fail "D-71: the $kind's embedded manifest does not declare PerMonitorV2 DPI awareness."

# D-91: each icon group as "id: widths", read from its RT_GROUP_ICON directory (a width byte
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
    fail "D-91: the $kind's icon groups are \"${icon_groups//$'\n'/; }\", not the app icon alone as group 1 at 16 to 256 px."
echo "Icon groups (id: sizes in px): ${icon_groups//$'\n'/; }."

if [[ $kind == "release exe" ]]; then
    # Windows keeps many strings as UTF-16, which llvm-strings does not read, so the exe is
    # also decoded as UTF-16LE at both byte alignments.
    strings=$(
        "$llvm_bin/llvm-strings" "$exe"
        # iconv fails on the half character an odd length leaves at the end; the rest is read.
        { iconv -f UTF-16LE -t UTF-8 -c "$exe" 2>/dev/null || true
          tail -c +2 "$exe" | iconv -f UTF-16LE -t UTF-8 -c 2>/dev/null || true; } |
            LC_ALL=C tr -c '[:print:]' '\n' | LC_ALL=C grep -a -E '.{4}'
    )
    host_paths=$(printf '%s\n' "$strings" | grep -a -i -F -e /Users/ -e /home/ -e '\Users\' -e '\.cargo\' -e ':\a\' || [[ $? -eq 1 ]])
    [[ -z $host_paths ]] ||
        fail "CHARTER 7 gate 3: the release exe holds $(printf '%s\n' "$host_paths" | wc -l | tr -d ' ') build-host paths, such as $(printf '%s\n' "$host_paths" | head -1)."
fi

size=$(wc -c <"$exe" | tr -d ' ')
echo "The $kind: $size bytes, $(printf '%s\n' "$dlls" | wc -l | tr -d ' ') DLLs, $(printf '%s\n' "$functions" | wc -l | tr -d ' ') imported functions."

exit "$failed"
