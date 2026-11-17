#!/bin/bash
set -euo pipefail
umask 077

# shellcheck source=scripts/ci-common.sh
source scripts/ci-common.sh
ci_context
install_dir=$(jq -er '.parameters.install_dir' "${TELETE_CONTEXT}")

deploy() {
    ci_verify_package
    command -v ffmpeg >/dev/null || ci_fail "Install FFmpeg."
    command -v ffprobe >/dev/null || ci_fail "Install FFprobe."
    ffprobe -version >/dev/null
    ffmpeg -hide_banner -encoders | awk '
        $2 == "libx264" { h264 = 1 }
        $2 == "aac" { aac = 1 }
        END { exit !(h264 && aac) }
    ' || ci_fail "Use an FFmpeg build with the libx264 and AAC encoders."

    mkdir -p -- "${install_dir}"
    local destination="${install_dir}/action"
    [[ ! -d ${destination} ]] || ci_fail "Remove the directory at ${destination} before installation."
    temporary=$(mktemp "${install_dir}/.action.XXXXXXXX")
    trap 'rm -f -- "$temporary"' EXIT
    install -m 0755 -- "${CI_BINARY}" "${temporary}"
    mv -f -- "${temporary}" "${destination}"
    printf 'Installed action %s from job %s at %s.\n' "${CI_VERSION}" "${CI_PACKAGE_JOB}" "${destination}"
}

deploy 2>&1 | tee "${CI_ARTIFACT_DIR}/deploy.log"
