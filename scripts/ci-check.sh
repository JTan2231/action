#!/bin/bash
set -euo pipefail
umask 077

# shellcheck source=scripts/ci-common.sh
source scripts/ci-common.sh
ci_context

build() {
    local changes
    changes=$(git status --porcelain=v1 --untracked-files=all --ignored=matching)
    [[ -z ${changes} ]] || ci_fail "The private checkout must be clean."
    command -v shellcheck >/dev/null || ci_fail "Install ShellCheck."
    command -v rsync >/dev/null || ci_fail "Install rsync."
    export PATH="${CARGO_HOME:-"${HOME}/.cargo"}/bin:${PATH}"
    command -v rustup >/dev/null || ci_fail "Install rustup."

    # Keep the build path and the modification times of unchanged files.
    local workspace="${CI_ROOT}/cache/source"
    mkdir -p -- "${workspace}"
    rsync -rclp --delete --delete-excluded --itemize-changes --exclude='/.git' ./ "${workspace}/"
    cd -- "${workspace}"

    local toolchain compiler compiler_key target
    toolchain=$(awk -F '"' '/^channel = "/ {print $2}' rust-toolchain.toml)
    [[ ${toolchain} =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || ci_fail "Set a Rust toolchain version."
    compiler=$(rustup run "${toolchain}" rustc -vV)
    target=$(awk '$1 == "host:" {print $2}' <<< "${compiler}")
    [[ ${target} =~ ^[a-z0-9_-]+$ ]] || ci_fail "Read the Rust host target."
    compiler_key=$(printf '%s\n' "${compiler}" | shasum -a 256)
    export CARGO_TARGET_DIR="${CI_ROOT}/cache/cargo/${compiler_key%% *}"
    printf '%s\nCargo target directory: %s\n' "${compiler}" "${CARGO_TARGET_DIR}"

    shellcheck --external-sources --severity=style --enable=all ci.sh scripts/*.sh
    rustup run "${toolchain}" cargo fmt --all --check
    rustup run "${toolchain}" cargo clippy --locked --offline --release --all-targets \
        --target "${target}" --verbose -- -D warnings
    rustup run "${toolchain}" cargo build --locked --offline --release --target "${target}" --verbose

    local result release version artifact_name digest
    result="${CARGO_TARGET_DIR}/${target}/release/action"
    release=$("${result}" --version)
    [[ ${release} =~ ^action\ ([0-9]+\.[0-9]+\.[0-9]+([-+][A-Za-z0-9.+-]+)?)$ ]] ||
        ci_fail "The binary must report a semantic version."
    version=${BASH_REMATCH[1]}
    artifact_name="action-${version}-${target}"
    staging=$(mktemp -d "${CI_ARTIFACT_DIR}/.package.XXXXXXXX")
    trap 'rm -rf -- "$staging"' EXIT
    install -m 0755 -- "${result}" "${staging}/${artifact_name}"
    digest=$(ci_checksum "${staging}/${artifact_name}")
    jq -n --arg job "${CI_JOB}" --arg commit "${CI_COMMIT}" --arg version "${version}" \
        --arg target "${target}" --arg binary "${artifact_name}" --arg sha256 "${digest}" '
        {schema: 1, job: $job, commit: $commit, version: $version,
         target: $target, binary: $binary, sha256: $sha256}
    ' > "${staging}/manifest.json"
    [[ ! -e ${CI_ARTIFACT_DIR}/package ]] || ci_fail "The package for ${CI_JOB} already exists."
    mv -- "${staging}" "${CI_ARTIFACT_DIR}/package"
    printf 'Saved %s in %s/package.\n' "${artifact_name}" "${CI_ARTIFACT_DIR}"
}

if [[ ${CI_PACKAGE_JOB} == "${CI_JOB}" ]]; then
    build 2>&1 | tee "${CI_ARTIFACT_DIR}/check.log"
else
    (
        ci_verify_package
        printf 'Verified the package for %s.\n' "${CI_PACKAGE_JOB}"
    ) 2>&1 | tee "${CI_ARTIFACT_DIR}/check.log"
fi
