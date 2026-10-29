#!/usr/bin/env bash
set -euo pipefail
set -m

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
run_state_file="$repo_root/.env.run-state"
action="${1:-}"
target="${2:-all}"

backend_pid=""
frontend_public_pid=""
frontend_admin_pid=""
accounts_host_pid=""
run_target=""

read_local_port_setting() {
    local setting_name="$1"
    local setting_value=""

    if [ -f "$repo_root/.env.local" ]; then
        setting_value="$(awk -F= -v wanted="$setting_name" '
            /^[[:space:]]*#/ { next }
            {
                key = $1
                gsub(/^[[:space:]]+|[[:space:]]+$/, "", key)
                sub(/^export[[:space:]]+/, "", key)
                value = $2
                gsub(/^[[:space:]]+|[[:space:]]+$/, "", value)
                if (key == wanted && value ~ /^[0-9]+$/) {
                    found = value
                }
            }
            END { if (found != "") print found }
        ' "$repo_root/.env.local")"
    fi

    printf '%s' "$setting_value"
}

configured_backend_port="${APP_SERVER__PORT:-${BACKEND_PORT:-$(read_local_port_setting APP_SERVER__PORT)}}"
backend_port="${configured_backend_port:-8080}"
frontend_public_port="${FRONTEND_PUBLIC_PORT:-3000}"
frontend_admin_port="${FRONTEND_ADMIN_PORT:-5173}"
accounts_host_port="${ACCOUNTS_HOST_PORT:-4174}"

write_run_state() {
    local temporary_state_file="$run_state_file.$$"

    umask 077
    {
        printf "RUNNER_PID='%s'\n" "$$"
        printf "RUN_TARGET='%s'\n" "$run_target"
        printf "BACKEND_PID='%s'\n" "$backend_pid"
        printf "FRONTEND_PUBLIC_PID='%s'\n" "$frontend_public_pid"
        printf "FRONTEND_ADMIN_PID='%s'\n" "$frontend_admin_pid"
        printf "ACCOUNTS_HOST_PID='%s'\n" "$accounts_host_pid"
        printf "BACKEND_PORT='%s'\n" "$backend_port"
        printf "FRONTEND_PUBLIC_PORT='%s'\n" "$frontend_public_port"
        printf "FRONTEND_ADMIN_PORT='%s'\n" "$frontend_admin_port"
        printf "ACCOUNTS_HOST_PORT='%s'\n" "$accounts_host_port"
    } > "$temporary_state_file"
    chmod 600 "$temporary_state_file"
    mv "$temporary_state_file" "$run_state_file"
}

signal_process_group() {
    local process_group_id="$1"
    local signal_name="$2"

    case "$process_group_id" in
        ''|*[!0-9]*) return 0 ;;
    esac

    if kill -0 -- "-$process_group_id" 2>/dev/null; then
        kill -"$signal_name" -- "-$process_group_id" 2>/dev/null || true
    elif kill -0 "$process_group_id" 2>/dev/null; then
        kill -"$signal_name" "$process_group_id" 2>/dev/null || true
    fi
}

remove_run_state() {
    local recorded_runner_pid=""

    if [ -f "$run_state_file" ]; then
        recorded_runner_pid="$(sed -n "s/^RUNNER_PID='\([0-9][0-9]*\)'$/\1/p" "$run_state_file")"
        if [ "$recorded_runner_pid" = "$$" ]; then
            rm -f "$run_state_file"
        fi
    fi
}

cleanup_run() {
    local exit_code=$?

    trap - EXIT INT TERM
    signal_process_group "$backend_pid" TERM
    signal_process_group "$frontend_public_pid" TERM
    signal_process_group "$frontend_admin_pid" TERM
    signal_process_group "$accounts_host_pid" TERM
    sleep 0.5
    signal_process_group "$backend_pid" KILL
    signal_process_group "$frontend_public_pid" KILL
    signal_process_group "$frontend_admin_pid" KILL
    signal_process_group "$accounts_host_pid" KILL
    wait 2>/dev/null || true
    remove_run_state

    exit "$exit_code"
}

handle_interrupt() {
    exit 130
}

handle_stop_request() {
    exit 0
}

validate_port() {
    local port_label="$1"
    local port_value="$2"

    case "$port_value" in
        ''|*[!0-9]*)
            echo "ERROR: $port_label port must be a number: $port_value" >&2
            exit 2
            ;;
    esac

    if [ "$port_value" -lt 1 ] || [ "$port_value" -gt 65535 ]; then
        echo "ERROR: $port_label port is outside 1-65535: $port_value" >&2
        exit 2
    fi
}

check_port_available() {
    local port_label="$1"
    local port_value="$2"
    local listener_pids=""
    local listener_pid=""

    listener_pids="$(lsof -nP -iTCP:"$port_value" -sTCP:LISTEN -t 2>/dev/null || true)"
    if [ -z "$listener_pids" ]; then
        return 0
    fi

    echo "ERROR: $port_label port $port_value is already in use:" >&2
    while IFS= read -r listener_pid; do
        [ -n "$listener_pid" ] || continue
        echo "  - pid=$listener_pid $(ps -p "$listener_pid" -o comm=,args= 2>/dev/null || true)" >&2
    done <<< "$listener_pids"
    echo "Stop that service or choose another port before running this stack." >&2
    return 1
}

require_frontend_tools() {
    if ! command -v pnpm >/dev/null 2>&1; then
        echo "ERROR: pnpm is required to start the frontend development servers." >&2
        exit 127
    fi
}

apply_process_limit_guard() {
    local process_headroom="${RUN_PROCESS_HEADROOM:-400}"
    local process_id_list=""
    local current_process_count=""
    local process_limit_ceiling=""

    case "$process_headroom" in
        0)
            return 0
            ;;
        ''|*[!0-9]*)
            echo "WARNING: RUN_PROCESS_HEADROOM must be a non-negative integer; skipping process limit guard."
            return 0
            ;;
    esac

    if process_id_list="$(pgrep -U "$(id -u)" 2>/dev/null)"; then
        current_process_count="$(printf '%s\n' "$process_id_list" | wc -l | tr -d ' ')"
    else
        echo "WARNING: Could not count current processes; skipping process limit guard."
        return 0
    fi

    case "$current_process_count" in
        ''|*[!0-9]*)
            echo "WARNING: Could not parse current process count; skipping process limit guard."
            return 0
            ;;
    esac

    process_limit_ceiling=$((current_process_count + process_headroom))
    if ulimit -u "$process_limit_ceiling" 2>/dev/null; then
        echo "Process guard: capped at $process_limit_ceiling processes (current $current_process_count + headroom $process_headroom)."
    else
        echo "WARNING: Could not set a process limit; continuing without the guard."
    fi
}

run_just_recipe() {
    (cd "$repo_root" && just "$@")
}

prepare_dependencies() {
    case "$run_target" in
        all)
            require_frontend_tools
            run_just_recipe web-install
            run_just_recipe web-admin-install
            (cd "$repo_root/examples/accounts-host" && CI=true pnpm install --frozen-lockfile --prefer-offline)
            ;;
        frontend|frontend-admin)
            require_frontend_tools
            run_just_recipe web-admin-install
            ;;
        frontend-public)
            require_frontend_tools
            run_just_recipe web-install
            ;;
        accounts-host)
            require_frontend_tools
            run_just_recipe accounts-ui-build
            (cd "$repo_root/examples/accounts-host" && CI=true pnpm install --frozen-lockfile --prefer-offline)
            ;;
        backend)
            ;;
        *)
            echo "ERROR: Unknown local run target: $run_target" >&2
            echo "Usage: just run [all|backend|frontend|frontend-public|accounts-host|docker]" >&2
            exit 2
            ;;
    esac
}

select_services() {
    start_backend="false"
    start_frontend_public="false"
    start_frontend_admin="false"
    start_accounts_host="false"

    case "$run_target" in
        all)
            start_backend="true"
            start_frontend_public="true"
            start_frontend_admin="true"
            start_accounts_host="true"
            ;;
        backend)
            start_backend="true"
            ;;
        frontend|frontend-admin)
            start_frontend_admin="true"
            ;;
        frontend-public)
            start_frontend_public="true"
            ;;
        accounts-host)
            start_accounts_host="true"
            ;;
        docker)
            ;;
        *)
            echo "ERROR: Unknown local run target: $run_target" >&2
            echo "Usage: just run [all|backend|frontend|frontend-public|accounts-host|docker]" >&2
            exit 2
            ;;
    esac
}

check_selected_ports() {
    command -v lsof >/dev/null 2>&1 || {
        echo "ERROR: lsof is required to check local development ports." >&2
        exit 127
    }

    if [ "$start_backend" = "true" ]; then
        validate_port "Accounts API" "$backend_port"
        check_port_available "Accounts API" "$backend_port"
    fi
    if [ "$start_frontend_public" = "true" ]; then
        validate_port "Public frontend" "$frontend_public_port"
        check_port_available "Public frontend" "$frontend_public_port"
    fi
    if [ "$start_frontend_admin" = "true" ]; then
        validate_port "Admin frontend" "$frontend_admin_port"
        check_port_available "Admin frontend" "$frontend_admin_port"
    fi
    if [ "$start_accounts_host" = "true" ]; then
        validate_port "Accounts UI host" "$accounts_host_port"
        check_port_available "Accounts UI host" "$accounts_host_port"
    fi
}

print_service_urls() {
    echo "Rust-study development services:"
    if [ "$start_backend" = "true" ]; then
        echo "  Accounts API readiness: http://127.0.0.1:$backend_port/ready"
    fi
    if [ "$start_frontend_public" = "true" ]; then
        echo "  Public site:             http://127.0.0.1:$frontend_public_port"
    fi
    if [ "$start_frontend_admin" = "true" ]; then
        echo "  Admin site:              http://127.0.0.1:$frontend_admin_port"
    fi
    if [ "$start_accounts_host" = "true" ]; then
        if [ "$start_backend" != "true" ]; then
            echo "  Accounts API upstream: http://127.0.0.1:$backend_port (must already be running)"
        fi
        echo "  Accounts UI example:     http://127.0.0.1:$accounts_host_port"
    fi
    echo "Stop these processes with: just down"
}

start_backend() {
    echo "Starting Rust accounts-service on 127.0.0.1:$backend_port"
    (
        cd "$repo_root"
        export APP_SERVER__PORT="$backend_port"
        exec cargo run -p tpl-accounts --bin accounts-service
    ) &
    backend_pid=$!
    write_run_state
}

start_public_frontend() {
    echo "Starting public Next.js frontend on 127.0.0.1:$frontend_public_port"
    (
        cd "$repo_root/frontend-public"
        export PORT="$frontend_public_port"
        export BACKEND_URL="http://127.0.0.1:$backend_port"
        exec pnpm dev
    ) &
    frontend_public_pid=$!
    write_run_state
}

start_admin_frontend() {
    echo "Starting admin Vite frontend on 127.0.0.1:$frontend_admin_port"
    (
        cd "$repo_root/frontend-admin"
        export BACKEND_PORT="$backend_port"
        export FRONTEND_ADMIN_PORT="$frontend_admin_port"
        exec pnpm dev
    ) &
    frontend_admin_pid=$!
    write_run_state
}

start_accounts_host() {
    echo "Starting accounts UI example on 127.0.0.1:$accounts_host_port"
    (
        cd "$repo_root/examples/accounts-host"
        export ACCOUNTS_URL="http://127.0.0.1:$backend_port"
        export ACCOUNTS_HOST_PORT="$accounts_host_port"
        exec pnpm dev
    ) &
    accounts_host_pid=$!
    write_run_state
}

wait_for_process_exit() {
    local process_pid=""
    local process_status=0

    while true; do
        for process_pid in "$backend_pid" "$frontend_public_pid" "$frontend_admin_pid" "$accounts_host_pid"; do
            [ -n "$process_pid" ] || continue
            if ! kill -0 "$process_pid" 2>/dev/null; then
                if wait "$process_pid"; then
                    process_status=0
                else
                    process_status=$?
                fi
                return "$process_status"
            fi
        done
        sleep 1
    done
}

run_local_services() {
    run_target="$target"
    select_services

    if [ "$run_target" = "docker" ]; then
        echo "Starting the Docker Compose stack. Use 'just stack-down' to stop it."
        (cd "$repo_root" && exec just stack-up)
        return $?
    fi

    if [ -f "$run_state_file" ]; then
        echo "ERROR: A managed development run already has state at $run_state_file." >&2
        echo "Run 'just down' before starting another local stack." >&2
        exit 1
    fi

    check_selected_ports
    prepare_dependencies
    apply_process_limit_guard
    trap cleanup_run EXIT
    trap handle_interrupt INT
    trap handle_stop_request TERM

    write_run_state
    print_service_urls

    if [ "$start_backend" = "true" ]; then
        start_backend
    fi
    if [ "$start_frontend_public" = "true" ]; then
        start_public_frontend
    fi
    if [ "$start_frontend_admin" = "true" ]; then
        start_admin_frontend
    fi
    if [ "$start_accounts_host" = "true" ]; then
        start_accounts_host
    fi

    wait_for_process_exit
}

stop_local_services() {
    local recorded_runner_pid=""
    local runner_command=""
    local wait_attempt=0

    case "$target" in
        docker)
            (cd "$repo_root" && exec just stack-down)
            return $?
            ;;
        all)
            ;;
        *)
            echo "ERROR: Unknown stop target: $target" >&2
            echo "Usage: just down [all|docker]" >&2
            return 2
            ;;
    esac

    if [ ! -f "$run_state_file" ]; then
        echo "No rust-study development run is recorded."
        return 0
    fi

    # shellcheck disable=SC1090
    source "$run_state_file"
    recorded_runner_pid="${RUNNER_PID:-}"
    case "$recorded_runner_pid" in
        ''|*[!0-9]*)
            echo "ERROR: Invalid runner PID in $run_state_file; refusing to signal processes." >&2
            return 1
            ;;
    esac

    runner_command="$(ps -p "$recorded_runner_pid" -o comm=,args= 2>/dev/null || true)"
    case "$runner_command" in
        *dev-services.sh*) ;;
        *)
            echo "No matching rust-study runner process; removing stale state only."
            rm -f "$run_state_file"
            return 0
            ;;
    esac

    echo "Stopping rust-study development services..."
    kill -TERM "$recorded_runner_pid"
    while kill -0 "$recorded_runner_pid" 2>/dev/null && [ "$wait_attempt" -lt 50 ]; do
        sleep 0.1
        wait_attempt=$((wait_attempt + 1))
    done

    if kill -0 "$recorded_runner_pid" 2>/dev/null; then
        echo "Runner did not stop promptly; stopping its recorded process groups."
        signal_process_group "${BACKEND_PID:-}" TERM
        signal_process_group "${FRONTEND_PUBLIC_PID:-}" TERM
        signal_process_group "${FRONTEND_ADMIN_PID:-}" TERM
        signal_process_group "${ACCOUNTS_HOST_PID:-}" TERM
        sleep 0.5
        signal_process_group "${BACKEND_PID:-}" KILL
        signal_process_group "${FRONTEND_PUBLIC_PID:-}" KILL
        signal_process_group "${FRONTEND_ADMIN_PID:-}" KILL
        signal_process_group "${ACCOUNTS_HOST_PID:-}" KILL
        kill -KILL "$recorded_runner_pid" 2>/dev/null || true
    fi

    rm -f "$run_state_file"
    echo "Rust-study development services stopped."
}

case "$action" in
    run)
        run_local_services
        ;;
    down)
        stop_local_services
        ;;
    *)
        echo "Usage: scripts/dev-services.sh [run|down] [all|backend|frontend|frontend-public|accounts-host|docker]" >&2
        exit 2
        ;;
esac
