#!/usr/bin/env bash
# Source after setting repo_root. Every hardware session needs its own window:
# an outer observer cannot stop a later open inside the same verifier.
require_live_iris() {
    # Explicit diagnostic authorization may acknowledge an exact set of prior
    # recovered session errors. Default qualification remains unchanged; the
    # diagnostic guard rejects any new error or system/memory/GPU fault and
    # pins the boot and all loaded module identities before each new session.
    if [[ -n "${V4L2_VA_ACKNOWLEDGED_SESSION_BASELINE:-}" ]]; then
        if ! python3 "$repo_root/tools/check-gpu-copy-session-baseline.py" \
            "$V4L2_VA_ACKNOWLEDGED_SESSION_BASELINE"; then
            echo "hardware_session=fail reason=acknowledged_session_baseline_changed"
            exit 1
        fi
        return
    fi
    local iris_state kernel_messages fault_pattern
    iris_state="$(awk '$1 == "qcom_iris" {print $5}' /proc/modules)" || {
        echo "hardware_session=fail reason=module_state_unavailable"
        exit 1
    }
    if [[ -n "$iris_state" && "$iris_state" != Live ]]; then
        echo "hardware_session=fail reason=iris_module_transition state=$iris_state"
        exit 1
    fi
    # Another client can fault the device while this verifier is compiling or
    # between sessions. A clean observation window cannot erase earlier faults.
    kernel_messages="$(journalctl -k -b --no-pager -o cat)" || {
        echo "hardware_session=fail reason=boot_kernel_log_unavailable"
        exit 1
    }
    if [[ -z "$kernel_messages" ]]; then
        echo "hardware_session=fail reason=boot_kernel_log_empty"
        exit 1
    fi
    fault_pattern='session error received|received system error|video hw is power on|Unhandled context fault|UBSAN:|KASAN:|BUG:|WARNING:|Internal error:|Oops:|Kernel panic|blocked for more than|watchdog:.*lockup'
    if [[ "$kernel_messages" =~ $fault_pattern ]]; then
        echo "hardware_session=fail reason=prior_boot_kernel_or_firmware_fault"
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
