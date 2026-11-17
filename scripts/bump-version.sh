#!/bin/bash
set -euo pipefail

bump=${1:-patch}
if [[ $# -gt 1 || ! ${bump} =~ ^(patch|minor|major)$ ]]; then
    printf '%s\n' 'Usage: ./ci.sh bump-version [patch|minor|major]' >&2
    exit 2
fi

cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
staging=$(mktemp -d .version.XXXXXXXX)
trap 'rm -rf -- "$staging"' EXIT
trap 'exit 1' HUP INT TERM

# Check both version records before you replace the source files.
awk -v bump="${bump}" -v staging="${staging}" '
    function fail(message) {
        print "bump-version: " message > "/dev/stderr"
        failed = 1
        exit 1
    }
    FILENAME == ARGV[1] {
        if ($0 ~ /^\[/) package = ($0 == "[package]")
        if (package && $0 ~ /^version = "/) {
            split($0, fields, /"/)
            old = fields[2]
            if (old !~ /^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)$/)
                fail("Use a major.minor.patch package version.")
            split(old, part, /[.]/)
            if (bump == "major") {
                part[1]++; part[2] = 0; part[3] = 0
            } else if (bump == "minor") {
                part[2]++; part[3] = 0
            } else part[3]++
            version = sprintf("%.0f.%.0f.%.0f", part[1], part[2], part[3])
            $0 = "version = \"" version "\""
            manifest_count++
        }
        print > (staging "/Cargo.toml")
        next
    }
    FILENAME == ARGV[2] {
        if ($0 == "[[package]]") package = 0
        if ($0 == "name = \"action\"") package = 1
        if (package && $0 ~ /^version = "/) {
            if ($0 != "version = \"" old "\"") fail("Make the version in Cargo.lock match Cargo.toml.")
            $0 = "version = \"" version "\""
            lock_count++
        }
        print > (staging "/Cargo.lock")
    }
    END {
        if (failed) exit 1
        if (manifest_count != 1 || lock_count != 1) fail("Set one version in each file.")
        print old " -> " version
    }
' Cargo.toml Cargo.lock

mv -f -- "${staging}/Cargo.toml" Cargo.toml
mv -f -- "${staging}/Cargo.lock" Cargo.lock
