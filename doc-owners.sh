#!/usr/bin/env bash
# STANDARDS "a doc comment sits directly on its own definition": fails when a commit keeps a
# doc line word for word but leaves it on an item the commit added, while the item it was on
# still exists, which is what inserting an item between a doc and its definition does.
#   ./doc-owners.sh [RANGE]   each commit in RANGE against its parent; RANGE defaults to the
#                             fold's LANES_FOLD_RANGE, else HEAD~1..HEAD.
set -euo pipefail
export LC_ALL=C
cd "$(dirname "$0")"
range=${1:-${LANES_FOLD_RANGE:-HEAD~1..HEAD}}

# "<owner>\t<doc line>" for each `///` line of PATH at REV whose text occurs once in the file;
# the owner is the name of the item, field or variant the doc block sits on.
owners() {
    git show "$1:$2" 2>/dev/null | awk '
        function name_of(line,    m) {
            if (match(line, /(fn|struct|enum|trait|type|const|static|mod|union)[ \t]+[A-Za-z0-9_]+/)) {
                m = substr(line, RSTART, RLENGTH); sub(/^[a-z]+[ \t]+/, "", m); return m
            }
            sub(/^pub(\([a-z]+\))?[ \t]+/, "", line)
            if (match(line, /^[A-Za-z_][A-Za-z0-9_]*/)) return substr(line, RSTART, RLENGTH)
            return line
        }
        {
            line = $0; sub(/^[ \t]+/, "", line)
            if (line ~ /^\/\/\//) { docs[++n] = line; next }
            if (n == 0) next
            if (skipping) { if (line ~ /\]$/) skipping = 0; next }
            if (line ~ /^#\[/) { if (line !~ /\]$/) skipping = 1; next }
            if (line ~ /^\/\//) next
            owner = name_of(line)
            for (i = 1; i <= n; i++) { seen[docs[i]]++; at[docs[i]] = owner }
            n = 0
        }
        END { for (d in seen) if (seen[d] == 1 && d !~ /^\/\/\/[ \t]*(#.*)?$/) print at[d] "\t" d }
    ' | sort
}

# Every item name in PATH at REV.
names() {
    git show "$1:$2" 2>/dev/null | awk '{
        while (match($0, /(fn|struct|enum|trait|type|const|static|mod|union)[ \t]+[A-Za-z0-9_]+/)) {
            m = substr($0, RSTART, RLENGTH); sub(/^[a-z]+[ \t]+/, "", m); print m
            $0 = substr($0, RSTART + RLENGTH)
        }
    }' | sort -u
}

failed=0
# On its own line, so set -e stops on a range git cannot list instead of checking nothing.
commits=$(git rev-list --reverse "$range")
for commit in $commits; do
    for path in $(git diff --name-only --diff-filter=M "$commit^" "$commit" -- '*.rs'); do
        moved=$(join -t $'\t' -1 2 -2 2 \
            <(owners "$commit^" "$path" | sort -t $'\t' -k2,2) \
            <(owners "$commit" "$path" | sort -t $'\t' -k2,2) \
            | awk -F '\t' '$2 != $3 { print $2 "\t" $3 "\t" $1 }')
        [ -n "$moved" ] || continue
        kept=$(names "$commit" "$path")
        earlier=$(names "$commit^" "$path")
        while IFS=$'\t' read -r was now doc; do
            grep -qxF "$was" <<< "$kept" || continue
            ! grep -qxF "$now" <<< "$earlier" || continue
            printf '%s %s: "%s" was on %s and is now on %s, which it added, and %s is still there\n' \
                "${commit:0:9}" "$path" "$doc" "$was" "$now" "$was"
            failed=1
        done <<< "$moved"
    done
done
exit "$failed"
