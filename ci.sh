#!/bin/bash
set -euo pipefail

cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
repo=$(pwd -P)
# shellcheck source=scripts/ci-common.sh
source scripts/ci-common.sh

usage() {
    cat <<'USAGE'
Usage: ./ci.sh [--follow]
       ./ci.sh submit [REVISION] [--follow]
       ./ci.sh release [REVISION] [--follow]
       ./ci.sh deploy --from JOB [--follow]
       ./ci.sh bump-version [patch|minor|major]
       ./ci.sh status [JOB]
       ./ci.sh wait JOB [--timeout SECONDS]
       ./ci.sh worktree PATH
       ./ci.sh queue COMMAND [ARGUMENTS]

Submit committed source to Telete.
Use release to install the checked binary.
Use deploy --from to install a saved binary.
USAGE
}

mode=${1:-submit}
case ${mode} in
    -h|--help) usage; exit ;;
    --follow) mode=submit ;;
    bump-version) shift; exec scripts/bump-version.sh "$@" ;;
    submit|release|deploy|queue) if [[ $# -gt 0 ]]; then shift; fi ;;
    status|wait|worktree|cancel|recover|pause|resume|maintenance|gates) ;;
    *) ci_fail "Unknown command ${mode}. Run ./ci.sh --help." ;;
esac

command -v telete >/dev/null || ci_fail "Install Telete."
command -v jq >/dev/null || ci_fail "Install jq."
common_dir=$(git rev-parse --path-format=absolute --git-common-dir)
common_dir=$(cd -- "${common_dir}" && pwd -P)
identity=$(printf '%s' "${common_dir}" | shasum -a 256)
identity=${identity%% *}
state_home=${ACTION_CI_ROOT:-${XDG_STATE_HOME:-"${HOME}/.local/state"}/action/ci}
install_dir=${ACTION_INSTALL_DIR:-"${HOME}/.local/bin"}
[[ ${state_home} == /* && ${install_dir} == /* ]] ||
    ci_fail "Use absolute paths for ACTION_CI_ROOT and ACTION_INSTALL_DIR."

umask 077
mkdir -p -- "${state_home}/${identity}"
ci_root=$(cd -- "${state_home}/${identity}" && pwd -P)
configured=$(mktemp "${ci_root}/config.XXXXXXXX")
trap 'rm -f -- "$configured"' EXIT
trap 'exit 130' INT
trap 'exit 1' HUP TERM

write_config() {
    jq --arg repo "${repo}" --arg root "${ci_root}" \
        --arg install_dir "${install_dir}" --arg candidate "$1" --arg mode "${mode}" '
        .repo = $repo |
        .state = ($root + "/telete") |
        .parameters = {ci_root: $root, install_dir: $install_dir, candidate: $candidate} |
        if $mode == "release" or $mode == "deploy" then . else del(.deploy) end
    ' telete.json > "${configured}"
}

run_telete() {
    telete --config "${configured}" "$@"
}

write_config ""
case ${mode} in
    submit|release)
        changes=$(git status --porcelain=v1 --untracked-files=all)
        [[ -z ${changes} ]] || ci_fail "Commit the changes before you submit CI."
        printf 'CI artifacts are in %s/artifacts.\n' "${ci_root}" >&2
        run_telete submit "$@"
        ;;
    deploy)
        [[ $# -ge 2 && $# -le 3 && $1 == --from && ${3:---follow} == --follow ]] ||
            ci_fail "Use ./ci.sh deploy --from JOB [--follow]."
        candidate=$2
        ci_job_id "${candidate}"
        manifest="${ci_root}/artifacts/${candidate}/package/manifest.json"
        [[ -f ${manifest} ]] || ci_fail "The package for ${candidate} is unavailable."
        commit=$(jq -er '.commit' "${manifest}")
        status=$(run_telete --json status "${candidate}")
        jq -e --arg commit "${commit}" '
            ((.resulting_commit // .commit // $commit) == $commit) and
            (.phase == "succeeded" or
             (.phase == "failed" and any(.stages[]?; .stage == "check" and .status == "succeeded")))
        ' >/dev/null <<< "${status}" || ci_fail "The check for ${candidate} must succeed before deployment."
        write_config "${candidate}"
        shift 2
        printf 'CI artifacts are in %s/artifacts.\n' "${ci_root}" >&2
        run_telete submit "${commit}" "$@"
        ;;
    worktree)
        [[ $# == 2 ]] || ci_fail "Use ./ci.sh worktree PATH."
        worktree_parent=$(dirname -- "$2")
        worktree_parent=$(cd -- "${worktree_parent}" && pwd -P)
        worktree_name=$(basename -- "$2")
        run_telete worktree "${worktree_parent}/${worktree_name}"
        ;;
    queue)
        run_telete "$@"
        ;;
    *)
        run_telete "$@"
        ;;
esac
