#!/usr/bin/env bash
# Source after setting repo_root. Every hardware session needs its own window:
# an outer observer cannot stop a later open inside the same verifier.
require_live_iris() {
    local iris_state
    iris_state="$(awk '$1 == "qcom_iris" {print $5}' /proc/modules)" || {
        echo "hardware_session=fail reason=module_state_unavailable"
        exit 1
    }
    if [[ -n "$iris_state" && "$iris_state" != Live ]]; then
        echo "hardware_session=fail reason=iris_module_transition state=$iris_state"
        exit 1
    fi
}

require_clean_kernel() {
    if ! python3 "$repo_root/tools/check-playback-performance.py" kernel --log "$1"; then
        echo "hardware_session=fail reason=kernel_errors_or_missing_observation log=$1"
        exit 1
    fi
}

run_kernel_checked() { # <log> <command...>
    local session_log="$1" session_status=0
    shift
    require_live_iris
    "$repo_root/tools/capture-iris-kernel-log.sh" -- "$@" > "$session_log" 2>&1 || session_status=$?
    require_clean_kernel "$session_log"
    return "$session_status"
}
