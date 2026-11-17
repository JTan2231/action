#!/bin/bash

ci_fail() {
    printf 'action CI: %s\n' "$*" >&2
    exit 1
}

ci_job_id() {
    [[ $1 =~ ^[A-Za-z0-9_-]+$ ]] || ci_fail "Invalid CI job ID: $1"
}

ci_context() {
    local context=${TELETE_CONTEXT:?Submit CI with ./ci.sh.}
    local current_commit
    CI_ROOT=$(jq -er '.parameters.ci_root' "${context}")
    CI_JOB=$(jq -er '.job_id' "${context}")
    CI_COMMIT=$(jq -er '.source.commit' "${context}")
    CI_PACKAGE_JOB=$(jq -er 'if .parameters.candidate == "" then .job_id else .parameters.candidate end' "${context}")
    ci_job_id "${CI_JOB}"
    ci_job_id "${CI_PACKAGE_JOB}"
    current_commit=$(git rev-parse HEAD)
    [[ ${current_commit} == "${CI_COMMIT}" ]] || ci_fail "The checkout must match the selected commit."
    CI_ARTIFACT_DIR="${CI_ROOT}/artifacts/${CI_JOB}"
    mkdir -p -- "${CI_ARTIFACT_DIR}"
}

ci_checksum() {
    local digest
    digest=$(shasum -a 256 < "$1") || return
    printf '%s\n' "${digest%% *}"
}

ci_verify_package() {
    local package manifest binary digest expected_digest release
    package="${CI_ROOT}/artifacts/${CI_PACKAGE_JOB}/package"
    manifest="${package}/manifest.json"
    [[ -f ${manifest} ]] || ci_fail "The package for ${CI_PACKAGE_JOB} is unavailable."
    jq -e --arg job "${CI_PACKAGE_JOB}" --arg commit "${CI_COMMIT}" '
        .schema == 1 and .job == $job and .commit == $commit
    ' "${manifest}" >/dev/null || ci_fail "The package must match job ${CI_PACKAGE_JOB} and commit ${CI_COMMIT}."

    binary=$(jq -er '.binary' "${manifest}")
    [[ ${binary} =~ ^action-[A-Za-z0-9.+_-]+$ ]] || ci_fail "Invalid package file name."
    CI_BINARY="${package}/${binary}"
    CI_VERSION=$(jq -er '.version' "${manifest}")
    [[ -x ${CI_BINARY} ]] || ci_fail "Package binary is missing: ${CI_BINARY}"
    digest=$(ci_checksum "${CI_BINARY}")
    expected_digest=$(jq -er '.sha256' "${manifest}")
    [[ ${digest} == "${expected_digest}" ]] || ci_fail "The package checksum must match ${CI_BINARY}."
    release=$("${CI_BINARY}" --version)
    [[ ${release} == "action ${CI_VERSION}" ]] || ci_fail "The package must report version ${CI_VERSION}."
}
