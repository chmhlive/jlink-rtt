#!/usr/bin/env bash

set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
log_info() { printf '[INFO] %s\n' "$*"; }
fail() { printf '[ERROR] %s\n' "$*" >&2; exit 1; }

log_info "Building jlink-rtt Rust binary in release mode..."
cargo build --release --manifest-path "${SCRIPT_DIR}/Cargo.toml"

BINARY_PATH="${SCRIPT_DIR}/target/release/jlink-rtt"
TMP_DIR="$(mktemp -d)"

cleanup() {
    rm -rf "${TMP_DIR}"
    # Kill any left-behind fake server from --stop test.
    kill "${FAKE_JLINK_PID:-}" 2>/dev/null || true
    pkill -f "python3.*simulate_ports" 2>/dev/null || true
}
trap cleanup EXIT

dump_debug() {
    local status=$?

    if ((status != 0)); then
        printf '[ERROR] Test failed with status %s\n' "${status}" >&2
        for file in \
            "${TMP_DIR}/rtt_output.log" \
            "${TMP_DIR}/captured_rtt.log" \
            "${TMP_DIR}/jlink.log" \
            "${TMP_DIR}/gdb.log" \
            "${TMP_DIR}/print_config.log" \
            "${TMP_DIR}/env_ignored.log" \
            "${TMP_DIR}/no_config.log" \
            "${TMP_DIR}/init_output.log" \
            "${TMP_DIR}/existing_init_output.log" \
            "${TMP_DIR}/no_config_output.log" \
            "${TMP_DIR}/no_config_serial_output.log" \
            "${TMP_DIR}/no_probe_output.log" \
            "${TMP_DIR}/capture_ok_output.log" \
            "${TMP_DIR}/match_timeout_output.log" \
            "${TMP_DIR}/python_simulator.log" \
            "${TMP_DIR}/stop_output.log"; do
            if [[ -f "${file}" ]]; then
                printf '\n--- %s ---\n' "${file}" >&2
                sed -n '1,160p' "${file}" >&2
            fi
        done
    fi

    cleanup
    exit "${status}"
}
trap dump_debug EXIT

mkdir -p "${TMP_DIR}/bin" "${TMP_DIR}/project/subdir"

# Fake host tools let the test cover orchestration without USB/J-Link hardware.
cat > "${TMP_DIR}/bin/JLinkGDBServer" <<'EOF'
#!/usr/bin/env bash
set -Eeuo pipefail

printf '%s\n' "$*" > "${JLINK_RTT_TEST_TMP}/jlink_args"
touch "${JLINK_RTT_TEST_TMP}/server_started"

# Parse ports from args
gdb_port=2331
rtt_port=19021

while [[ $# -gt 0 ]]; do
    case "$1" in
        -port)
            gdb_port="$2"
            shift 2
            ;;
        -RTTTelnetPort)
            rtt_port="$2"
            shift 2
            ;;
        *)
            shift
            ;;
    esac
done

# Pass ports to python script via environment variables to avoid quotes and newlines issues in bash -c
export GDB_PORT_ENV="${gdb_port}"
export RTT_PORT_ENV="${rtt_port}"

# Start python simulator in background
python3 -c '
# simulate_ports
import os, socket, time, threading
gdb_port = int(os.environ["GDB_PORT_ENV"])
rtt_port = int(os.environ["RTT_PORT_ENV"])

def monitor_parent():
    parent_pid = os.getppid()
    print(f"[DEBUG] Python PID: {os.getpid()}, Parent PID: {parent_pid}")
    while True:
        try:
            with open(f"/proc/{parent_pid}/stat", "r") as f:
                stat = f.read().split()
                if stat[2] == "Z":
                    print(f"[DEBUG] Parent {parent_pid} became zombie. Exiting.")
                    os._exit(0)
        except IOError:
            print(f"[DEBUG] Parent {parent_pid} stat not found. Exiting.")
            os._exit(0)
        time.sleep(0.05)

def listen_gdb(port):
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("127.0.0.1", port))
    s.listen(5)
    while True:
        try:
            conn, addr = s.accept()
            conn.close()
        except Exception:
            break

def handle_rtt_client(conn, data):
    try:
        conn.sendall(data)
        conn.settimeout(0.2)
        downlink_file = os.environ.get("JLINK_RTT_TEST_TMP", "/tmp") + "/downlink_received.log"
        start_t = time.time()
        while time.time() - start_t < 5:
            try:
                chunk = conn.recv(1024)
                if chunk:
                    with open(downlink_file, "ab") as f:
                        f.write(chunk)
                else:
                    break
            except socket.timeout:
                continue
        conn.close()
    except Exception:
        pass

def listen_rtt(port, data):
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("127.0.0.1", port))
    s.listen(5)
    while True:
        try:
            conn, addr = s.accept()
            t = threading.Thread(target=handle_rtt_client, args=(conn, data))
            t.daemon = True
            t.start()
        except Exception:
            break

# Monitor parent thread
t_mon = threading.Thread(target=monitor_parent)
t_mon.daemon = True
t_mon.start()

# Listen on GDB port in thread
t = threading.Thread(target=listen_gdb, args=(gdb_port,))
t.daemon = True
t.start()

# Listen on RTT port; payload overridable per-test (e.g. residual no-newline tail)
payload = b"boot line\nApplication started\n"
payload_file = os.environ.get("JLINK_RTT_TEST_PAYLOAD")
if payload_file:
    with open(payload_file, "rb") as f:
        payload = f.read()
listen_rtt(rtt_port, payload)
' > "${JLINK_RTT_TEST_TMP}/python_simulator.log" 2>&1 &

cleanup_srv() {
    rm -f "${JLINK_RTT_TEST_TMP}/server_started"
    exit 0
}
trap cleanup_srv TERM INT

while true; do
    sleep 1
done
EOF

cat > "${TMP_DIR}/bin/nc" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF

chmod +x "${TMP_DIR}/bin/JLinkGDBServer" "${TMP_DIR}/bin/nc"

# Fake JLinkExe for device database resolution (used by --init fuzzy matching) and reset orchestration
cat > "${TMP_DIR}/bin/JLinkExe" <<'JLEOF'
#!/usr/bin/env bash
set -Eeuo pipefail

printf '%s\n' "$*" >> "${JLINK_RTT_TEST_TMP:-/tmp}/jlink_run_args" 2>/dev/null || true

printf 'SEGGER J-Link Commander V9.99a (Compiled Jan 1 2026 00:00:00)\n'
if [[ "${*}" == *"/dev/null"* || "${*}" == *"/dev/zero"* ]]; then
    exit 0
fi
# Parse -CommandFile or -CommanderScript to find the script, then extract ExpDevList target path.
cmd_file=""
for arg in "$@"; do
    if [[ "${arg}" == "-CommandFile" || "${arg}" == "-CommanderScript" ]]; then continue; fi
    if [[ -f "${arg}" ]]; then cmd_file="${arg}"; break; fi
done
if [[ -n "${cmd_file}" ]]; then
    # Copy script content to verification log before it gets cleaned up by orchestrator
    cat "${cmd_file}" >> "${JLINK_RTT_TEST_TMP:-/tmp}/jlink_run_commands" 2>/dev/null || true
    
    csv_path="$(sed -n 's/^ExpDevList[[:space:]]\+//p' "${cmd_file}" | head -1)"
    if [[ -n "${csv_path}" ]]; then
        cat > "${csv_path}" <<CSV
"Manufacturer", "Device", "Core", {Flash areas}, {RAM areas}
"Nordic Semi", "nRF52840_xxAA", "Cortex-M4", { {0x00000000, 0x00100000} }, {0x20000000, 0x00040000}
"Nordic Semi", "nRF52833_xxAA", "Cortex-M4", { {0x00000000, 0x00080000} }, {0x20000000, 0x00020000}
"Nordic Semi", "nRF52832_xxAA", "Cortex-M4", { {0x00000000, 0x00080000} }, {0x20000000, 0x00010000}
"ST", "STM32F407IG", "Cortex-M4", { {0x08000000, 0x00100000} }, {0x20000000, 0x00020000}
CSV
    fi
fi
exit 0
JLEOF
chmod +x "${TMP_DIR}/bin/JLinkExe"

cat > "${TMP_DIR}/project/.prj.env" <<EOF
JLINK_DEVICE=NRF52840_XXAA
JLINK_IF=SWD
JLINK_SPEED=4000
LISTEN_HOST=127.0.0.1
GDB_PORT=32331
RTT_PORT=39021
RTT_READY_TIMEOUT=2
JLINK_LOG_FILE=${TMP_DIR}/jlink.log
JLINK_GDB_LOG_FILE=${TMP_DIR}/gdb.log
EOF

OUTPUT_FILE="${TMP_DIR}/rtt_output.log"
OUT_FILE="${TMP_DIR}/captured_rtt.log"

log_info "Test 1: Run RTT capture with matching pattern exit..."
(
    cd "${TMP_DIR}/project/subdir"
    JLINK_RTT_TEST_TMP="${TMP_DIR}" \
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --match "Application started" \
        --timeout 3 \
        --out "${OUT_FILE}" \
        > "${OUTPUT_FILE}" 2>&1
) && pattern_ok=1 || pattern_ok=0

((pattern_ok == 1)) || fail "Pattern-triggered capture did not exit 0."

grep -Fq 'Application started' "${OUTPUT_FILE}" || fail "RTT output was not forwarded."
grep -Fq 'Application started' "${OUT_FILE}" || fail "RTT output was not saved."
grep -Fq 'Matched RTT pattern: Application started' "${OUTPUT_FILE}" || fail "Pattern-triggered match message missing."
grep -Fq -- '-device NRF52840_XXAA' "${TMP_DIR}/jlink_args" || fail "JLink device argument is missing."
grep -Fq -- '-RTTTelnetPort 39021' "${TMP_DIR}/jlink_args" || fail "RTT port argument is missing."
grep -Fq 'r' "${TMP_DIR}/jlink_run_commands" || fail "JLink Commander reset command (r) is missing."
grep -Fq 'g' "${TMP_DIR}/jlink_run_commands" || fail "JLink Commander go command (g) is missing."

# --- timed capture self-exit (unified --timeout without --match) ---
pkill -f "python3.*simulate_ports" 2>/dev/null || true
log_info "Test 1b: Timed capture self-exit..."
TIMED_OUT_LOG="${TMP_DIR}/timed_output.log"
(
    cd "${TMP_DIR}/project/subdir"
    JLINK_RTT_TEST_TMP="${TMP_DIR}" \
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --timeout 2 \
        --out "${TIMED_OUT_LOG}" \
        > "${TIMED_OUT_LOG}.stdout" 2>&1
) && timed_ok=1 || timed_ok=0

((timed_ok == 1)) || fail "Timed capture did not exit 0 after --timeout elapsed."

grep -Fq 'RTT capture duration elapsed' "${TIMED_OUT_LOG}.stdout" \
    || fail "Timed capture self-exit message missing."
grep -Fq 'boot line' "${TIMED_OUT_LOG}" || fail "Timed capture did not save streamed data."

# --- invalid --timeout must fail loudly (no silent unlimited fallback) ---
log_info "Test 1c: Invalid --timeout value rejected..."
INVALID_OUT="${TMP_DIR}/invalid_timeout.log"
(
    cd "${TMP_DIR}/project/subdir"
    JLINK_RTT_TEST_TMP="${TMP_DIR}" \
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --timeout abc \
        --out "${TMP_DIR}/never.log" \
        > "${INVALID_OUT}" 2>&1
) && fail "Invalid --timeout should exit non-zero." || true

grep -Fq "Invalid --timeout / RTT_TIMEOUT value 'abc'" "${INVALID_OUT}" \
    || fail "Invalid --timeout error message missing."

# --- --ready-timeout CLI override flows into resolved config ---
log_info "Test 1d: --ready-timeout CLI override..."
READY_OUT="${TMP_DIR}/ready_timeout.log"
(
    cd "${TMP_DIR}/project/subdir"
    JLINK_RTT_TEST_TMP="${TMP_DIR}" \
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --ready-timeout 7 \
        --print-config \
        > "${READY_OUT}" 2>&1
) || fail "print-config with --ready-timeout failed."

grep -Fq 'RTT_READY_TIMEOUT=7' "${READY_OUT}" \
    || fail "--ready-timeout did not override RTT_READY_TIMEOUT."

# --- --match-timeout sets the match wait limit, overriding --timeout (TODO canonical) ---
log_info "Test 1e: --match-timeout match wait..."
MATCH_TIMEOUT_OUT="${TMP_DIR}/match_timeout.log"
MATCH_TIMEOUT_DATA="${TMP_DIR}/match_timeout_data.log"
(
    cd "${TMP_DIR}/project/subdir"
    JLINK_RTT_TEST_TMP="${TMP_DIR}" \
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --match "Application started" \
        --match-timeout 3 \
        --out "${MATCH_TIMEOUT_DATA}" \
        > "${MATCH_TIMEOUT_OUT}" 2>&1
) || fail "--match-timeout capture did not exit 0."

grep -Fq 'Matched RTT pattern: Application started' "${MATCH_TIMEOUT_OUT}" \
    || fail "--match-timeout match message missing."
grep -Fq 'Application started' "${MATCH_TIMEOUT_DATA}" \
    || fail "--match-timeout capture did not save output."

# --- alias --rtt-timeout flows into resolved config ---
log_info "Test 1e2: --rtt-timeout alias..."
RTT_ALIAS_OUT="${TMP_DIR}/rtt_timeout_alias.log"
(
    cd "${TMP_DIR}/project/subdir"
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --match "x" \
        --rtt-timeout 9 \
        --print-config \
        > "${RTT_ALIAS_OUT}" 2>&1
) || fail "print-config with --rtt-timeout failed."

grep -Fq 'RTT_TIMEOUT=9' "${RTT_ALIAS_OUT}" \
    || fail "--rtt-timeout alias did not set RTT_TIMEOUT."

# --- invalid --match-timeout value rejected loudly ---
log_info "Test 1e3: Invalid --match-timeout rejected..."
BAD_MT_OUT="${TMP_DIR}/bad_match_timeout.log"
(
    cd "${TMP_DIR}/project/subdir"
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --match "x" \
        --match-timeout abc \
        --print-config \
        > "${BAD_MT_OUT}" 2>&1
) && fail "Invalid --match-timeout should exit non-zero." || true

grep -Fq "Invalid --match-timeout value 'abc'" "${BAD_MT_OUT}" \
    || fail "Invalid --match-timeout error message missing."

# --- early disconnect inside the capture window fails closed ---
pkill -f "python3.*simulate_ports" 2>/dev/null || true
log_info "Test 1f: Early disconnect fails closed..."
EARLY_OUT="${TMP_DIR}/early_close.log"
(
    cd "${TMP_DIR}/project/subdir"
    JLINK_RTT_TEST_TMP="${TMP_DIR}" \
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --timeout 30 \
        --out "${EARLY_OUT}" \
        > "${EARLY_OUT}.stdout" 2>&1
) && fail "Early disconnect inside the capture window should exit non-zero." || true

grep -Fq 'capture is incomplete' "${EARLY_OUT}.stdout" \
    || fail "Early disconnect fail-closed message missing."
grep -Fq 'boot line' "${EARLY_OUT}" || fail "Early disconnect did not save streamed data."

# --- --timeout 0 normalizes to unset (0 and unset are equivalent) ---
log_info "Test 1g: --timeout 0 normalizes to continuous stream..."
ZERO_OUT="${TMP_DIR}/zero_timeout.log"
(
    cd "${TMP_DIR}/project/subdir"
    JLINK_RTT_TEST_TMP="${TMP_DIR}" \
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --timeout 0 \
        --print-config \
        > "${ZERO_OUT}" 2>&1
) || fail "print-config with --timeout 0 failed."

grep -Fxq 'RTT_TIMEOUT=' "${ZERO_OUT}" \
    || fail "--timeout 0 should normalize to an empty RTT_TIMEOUT line."

# --- residual no-newline tail is matched on deadline ---
pkill -f "python3.*simulate_ports" 2>/dev/null || true
log_info "Test 1h: Residual no-newline tail matched on deadline..."
printf 'boot line\nRESIDUAL READY' > "${TMP_DIR}/residual_payload.bin"
RESIDUAL_OUT="${TMP_DIR}/residual_output.log"
(
    cd "${TMP_DIR}/project/subdir"
    JLINK_RTT_TEST_TMP="${TMP_DIR}" \
    JLINK_RTT_TEST_PAYLOAD="${TMP_DIR}/residual_payload.bin" \
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --match "RESIDUAL READY" \
        --timeout 2 \
        --out "${TMP_DIR}/residual_capture.log" \
        > "${RESIDUAL_OUT}" 2>&1
) && residual_ok=1 || residual_ok=0

((residual_ok == 1)) || fail "Residual no-newline pattern was not matched on deadline."

grep -Fq 'Matched RTT pattern: RESIDUAL READY' "${RESIDUAL_OUT}" \
    || fail "Residual tail match message missing."
grep -Fq 'RESIDUAL READY' "${TMP_DIR}/residual_capture.log" \
    || fail "Residual tail was not saved to out file."

# --- empty --match pattern is rejected at resolve stage ---
log_info "Test 1i: Empty --match pattern rejected..."
EMPTY_MATCH_OUT="${TMP_DIR}/empty_match.log"
(
    cd "${TMP_DIR}/project/subdir"
    JLINK_RTT_TEST_TMP="${TMP_DIR}" \
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --match "" \
        --timeout 3 \
        --out "${TMP_DIR}/never2.log" \
        > "${EMPTY_MATCH_OUT}" 2>&1
) && fail "Empty --match pattern should exit non-zero." || true

grep -Fq 'cannot be empty' "${EMPTY_MATCH_OUT}" \
    || fail "Empty --match error message missing."

# --- post-match context lines from the same read batch are captured ---
# 前提: 三行 payload 必须在同一次 read 中到齐 (单次 sendall + loopback 下稳定,
# 38B 远小于 1024B 读取块); 若 CI 分段导致失败, 先核对这一测试假设再查产品代码
pkill -f "python3.*simulate_ports" 2>/dev/null || true
log_info "Test 1j: Post-match context lines captured..."
printf 'line1\nMATCHED\ntrailing tail after match\n' > "${TMP_DIR}/postmatch_payload.bin"
POSTMATCH_OUT="${TMP_DIR}/postmatch_output.log"
(
    cd "${TMP_DIR}/project/subdir"
    JLINK_RTT_TEST_TMP="${TMP_DIR}" \
    JLINK_RTT_TEST_PAYLOAD="${TMP_DIR}/postmatch_payload.bin" \
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --match "MATCHED" \
        --timeout 5 \
        --out "${TMP_DIR}/postmatch_capture.log" \
        > "${POSTMATCH_OUT}" 2>&1
) && postmatch_ok=1 || postmatch_ok=0

((postmatch_ok == 1)) || fail "Post-match capture did not exit 0."

grep -Fq 'trailing tail after match' "${TMP_DIR}/postmatch_capture.log" \
    || fail "Post-match context lines were dropped (payload must arrive in a single read; check the test assumption)."

# --- zero --ready-timeout is rejected as meaningless ---
log_info "Test 1k: Zero --ready-timeout rejected..."
ZERO_READY_OUT="${TMP_DIR}/zero_ready.log"
(
    cd "${TMP_DIR}/project/subdir"
    JLINK_RTT_TEST_TMP="${TMP_DIR}" \
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --ready-timeout 0 \
        --print-config \
        > "${ZERO_READY_OUT}" 2>&1
) && fail "Zero --ready-timeout should exit non-zero." || true

grep -Fq 'at least 1' "${ZERO_READY_OUT}" \
    || fail "Zero --ready-timeout error message missing."

# --- no-newline flood: the 1 MiB cap must clear the buffer without losing bytes ---
pkill -f "python3.*simulate_ports" 2>/dev/null || true
log_info "Test 1l: No-newline flood preserves every byte across cap flushes..."
head -c 1049600 /dev/zero | tr '\0' 'A' > "${TMP_DIR}/flood_payload.bin"
FLOOD_OUT="${TMP_DIR}/flood_output.log"
(
    cd "${TMP_DIR}/project/subdir"
    JLINK_RTT_TEST_TMP="${TMP_DIR}" \
    JLINK_RTT_TEST_PAYLOAD="${TMP_DIR}/flood_payload.bin" \
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --match "ZZZNOTPRESENT" \
        --timeout 20 \
        --out "${TMP_DIR}/flood_capture.log" \
        > "${FLOOD_OUT}" 2>&1
) && fail "Flood capture without the pattern should exit non-zero." || true

payload_bytes=$(wc -c < "${TMP_DIR}/flood_payload.bin")
captured_bytes=$(wc -c < "${TMP_DIR}/flood_capture.log")
(( captured_bytes == payload_bytes )) \
    || fail "Flood capture lost bytes: ${captured_bytes}/${payload_bytes}."

# --- 关键词跨越 1 MiB 兜底切点时, 保留的尾字节必须仍能命中 ---
# 读取块上限 1024B, 故切点 C ∈ [1048576, 1049599]; 关键词长 1036B 且起点固定在
# 1048572 (= 1MiB-4), 保证 C 必落在关键词内部, 且其头部 (C-起点 ≤ 1027B)
# 不超过保留长度 keep = 1035B; 若兜底退回整体 clear(), 本用例必然失配
pkill -f "python3.*simulate_ports" 2>/dev/null || true
log_info "Test 1m: Pattern straddling the 1 MiB cap boundary still matches..."
STRADDLE_KEY="X$(head -c 1035 /dev/zero | tr '\0' 'K')"
head -c 1048572 /dev/zero | tr '\0' 'A' > "${TMP_DIR}/straddle_payload.bin"
printf '%s' "${STRADDLE_KEY}" >> "${TMP_DIR}/straddle_payload.bin"
head -c 4096 /dev/zero | tr '\0' 'B' >> "${TMP_DIR}/straddle_payload.bin"
STRADDLE_OUT="${TMP_DIR}/straddle_output.log"
(
    cd "${TMP_DIR}/project/subdir"
    JLINK_RTT_TEST_TMP="${TMP_DIR}" \
    JLINK_RTT_TEST_PAYLOAD="${TMP_DIR}/straddle_payload.bin" \
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --match "${STRADDLE_KEY}" \
        --timeout 3 \
        --out "${TMP_DIR}/straddle_capture.log" \
        > "${STRADDLE_OUT}" 2>&1
) && straddle_ok=1 || straddle_ok=0

((straddle_ok == 1)) \
    || fail "Straddling pattern was not matched (cap flush must keep pattern-1 tail bytes)."

grep -Fq 'Matched RTT pattern' "${STRADDLE_OUT}" \
    || fail "Straddling pattern match message missing."
grep -Fq -- "${STRADDLE_KEY}" "${TMP_DIR}/straddle_capture.log" \
    || fail "Straddling pattern was not saved to out file intact."

# --- pattern-triggered timeout / close ---
pkill -f "python3.*simulate_ports" 2>/dev/null || true
log_info "Test 2: Non-existent pattern mismatch exit..."
MATCH_TIMEOUT_OUT="${TMP_DIR}/match_timeout_output.log"
(
    cd "${TMP_DIR}/project/subdir"
    JLINK_RTT_TEST_TMP="${TMP_DIR}" \
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --match "NONEXISTENT_PATTERN" \
        --timeout 2 \
        > "${MATCH_TIMEOUT_OUT}" 2>&1
) && fail "Pattern-triggered timeout/close should exit non-zero." || true

(grep -Fq 'Timed out waiting for RTT pattern' "${MATCH_TIMEOUT_OUT}" || grep -Fq 'closed before pattern' "${MATCH_TIMEOUT_OUT}") \
    || fail "Pattern-triggered timeout/close message missing."

# --- print config ---
log_info "Test 3: Print config verification..."
PRINT_CONFIG="${TMP_DIR}/print_config.log"
(
    cd "${TMP_DIR}/project/subdir"
    JLINK_RTT_TEST_TMP="${TMP_DIR}" \
    PATH="${TMP_DIR}/bin:${PATH}" \
    JLINK_DEVICE=ENV_DEVICE \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --print-config \
        --device CLI_DEVICE \
        > "${PRINT_CONFIG}" 2>&1
)

grep -Fq 'CONFIG_FILE='"${TMP_DIR}"'/project/.prj.env' "${PRINT_CONFIG}" || fail "Config file was not discovered within project root."
grep -Fq 'JLINK_DEVICE=CLI_DEVICE' "${PRINT_CONFIG}" || fail "Command line did not override config."

# JLINK_DEVICE=ENV_DEVICE is intentional: RTT settings must not be overridden by env vars.
log_info "Test 4: Ignore environmental variables override..."
ENV_IGNORED="${TMP_DIR}/env_ignored.log"
(
    cd "${TMP_DIR}/project/subdir"
    PATH="${TMP_DIR}/bin:${PATH}" \
    JLINK_DEVICE=ENV_DEVICE \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --print-config \
        > "${ENV_IGNORED}" 2>&1
)

grep -Fq 'JLINK_DEVICE=NRF52840_XXAA' "${ENV_IGNORED}" || fail "Config JLINK_DEVICE was not used."
if grep -Fq 'JLINK_DEVICE=ENV_DEVICE' "${ENV_IGNORED}"; then
    fail "Environment JLINK_DEVICE unexpectedly overrode config."
fi

# Non-git directories only check the current directory unless --project-root is explicit.
log_info "Test 5: Scope limit config discovery..."
mkdir -p "${TMP_DIR}/outside/subdir"
cat > "${TMP_DIR}/outside/.prj.env" <<EOF
JLINK_DEVICE=SHOULD_NOT_LOAD
EOF

NO_CONFIG="${TMP_DIR}/no_config.log"
(
    cd "${TMP_DIR}/outside/subdir"
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" --print-config > "${NO_CONFIG}" 2>&1
)

if grep -Fq 'JLINK_DEVICE=SHOULD_NOT_LOAD' "${NO_CONFIG}"; then
    fail "Non-git search escaped current directory without an explicit project root."
fi

# --- --init mode ---
log_info "Test 6: Init configuration file mode..."
INIT_DIR="${TMP_DIR}/init_project"
mkdir -p "${INIT_DIR}"
INIT_CONFIG="${INIT_DIR}/.prj.env"
INIT_OUT="${TMP_DIR}/init_output.log"

(
    cd "${INIT_DIR}"
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${INIT_DIR}" \
        --init \
        --device NRF52840_XXAA \
        > "${INIT_OUT}" 2>&1
)

grep -Fq 'JLINK_DEVICE=nRF52840_xxAA' "${INIT_CONFIG}" || fail "--init did not write JLINK_DEVICE."
grep -Fq 'JLINK_IF=SWD' "${INIT_CONFIG}" || fail "--init did not write JLINK_IF."
grep -Fq 'JLINK_SPEED=4000' "${INIT_CONFIG}" || fail "--init did not write JLINK_SPEED."
grep -Fq '# --- J-Link RTT 调试配置 ---' "${INIT_CONFIG}" || fail "--init did not write Chinese comment header."
grep -Fq 'Created config:' "${INIT_OUT}" || fail "--init did not print config created message."

# --- --init append mode ---
log_info "Test 6b: Init append configuration to existing .prj.env..."
APPEND_DIR="${TMP_DIR}/append_project"
mkdir -p "${APPEND_DIR}"
APPEND_CONFIG="${APPEND_DIR}/.prj.env"
cat > "${APPEND_CONFIG}" <<EOF
BOARD_TARGET="mr01/nrf52840"
NCS_VERSION="v3.4.0"
EOF
APPEND_OUT="${TMP_DIR}/append_output.log"

(
    cd "${APPEND_DIR}"
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${APPEND_DIR}" \
        --init \
        --device NRF52840_XXAA \
        > "${APPEND_OUT}" 2>&1
)

grep -Fq 'BOARD_TARGET="mr01/nrf52840"' "${APPEND_CONFIG}" || fail "Existing BOARD_TARGET was overwritten."
grep -Fq 'JLINK_DEVICE=nRF52840_xxAA' "${APPEND_CONFIG}" || fail "Appended config did not write JLINK_DEVICE."
grep -Fq 'Appended config to:' "${APPEND_OUT}" || fail "--init did not print appended message."

# --- no-config message ---
log_info "Test 7: Informative guide on no config file..."
NO_CFG_DIR="${TMP_DIR}/no_config_project"
mkdir -p "${NO_CFG_DIR}"
NO_CFG_OUT="${TMP_DIR}/no_config_output.log"

(
    cd "${NO_CFG_DIR}"
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${NO_CFG_DIR}" \
        > "${NO_CFG_OUT}" 2>&1
)

grep -Fq 'No .prj.env found' "${NO_CFG_OUT}" || fail "No-config did not print missing config message."
grep -Fq 'Scan the project for the DEVICE name' "${NO_CFG_OUT}" || fail "No-config did not print scan-project hint."
grep -Fq -- '--init --device' "${NO_CFG_OUT}" || fail "No-config did not print --init command hint."

# --- no-config with single J-Link probe (auto-detect serial) ---
log_info "Test 8: Auto-detect serial in init hint..."
cat > "${TMP_DIR}/bin/lsusb" <<'EOF'
#!/usr/bin/env bash
if [[ "${1:-}" == "-v" ]]; then
    printf 'Bus 001 Device 004: ID 1366:1024 SEGGER J-Link\n'
    printf '  iSerial                 3 000683041131\n'
    exit 0
fi
printf 'Bus 001 Device 004: ID 1366:1024 SEGGER J-Link\n'
exit 0
EOF
chmod +x "${TMP_DIR}/bin/lsusb"

NO_CFG_SERIAL_OUT="${TMP_DIR}/no_config_serial_output.log"
(
    cd "${NO_CFG_DIR}"
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${NO_CFG_DIR}" \
        > "${NO_CFG_SERIAL_OUT}" 2>&1
)

grep -Fq -- '--serial 000683041131' "${NO_CFG_SERIAL_OUT}" || fail "No-config did not auto-detect serial in init command."

# --- no_probe warning (lsusb returns nothing) ---
pkill -f "python3.*simulate_ports" 2>/dev/null || true
log_info "Test 9: Warning when no USB probes found..."
cat > "${TMP_DIR}/bin/lsusb" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
chmod +x "${TMP_DIR}/bin/lsusb"

NO_PROBE_OUT="${TMP_DIR}/no_probe_output.log"
(
    cd "${TMP_DIR}/project/subdir"
    JLINK_RTT_TEST_TMP="${TMP_DIR}" \
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --match "Application started" \
        --timeout 3 \
        > "${NO_PROBE_OUT}" 2>&1
)

grep -Fq 'No SEGGER/J-Link USB device detected' "${NO_PROBE_OUT}" || fail "Missing USB not detected warning."

# --- --init with existing config should die with hint ---
log_info "Test 10: Fail --init on existing config file..."
EXISTING_INIT_OUT="${TMP_DIR}/existing_init_output.log"
(
    cd "${TMP_DIR}/project/subdir"
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --init \
        --device NRF52840_XXAA \
        > "${EXISTING_INIT_OUT}" 2>&1 || true
)
if ! grep -Fq 'Config file already exists' "${EXISTING_INIT_OUT}"; then
    fail "--init on existing config did not report conflict."
fi

# --- --stop kills running session ---
pkill -f "python3.*simulate_ports" 2>/dev/null || true
log_info "Test 11: Stop command kills running session..."
STOP_OUT="${TMP_DIR}/stop_output.log"

# Start fake JLinkGDBServer in background with matching ports.
JLINK_RTT_TEST_TMP="${TMP_DIR}" PATH="${TMP_DIR}/bin:${PATH}" \
"${TMP_DIR}/bin/JLinkGDBServer" -port 32331 -RTTTelnetPort 39021 &
FAKE_JLINK_PID=$!

# Wait for fake server to be ready (up to 3s).
for _ in $(seq 1 30); do
    [[ -f "${TMP_DIR}/server_started" ]] && break
    sleep 0.1
done

# First --stop should kill the server by port match, exit 0.
(
    cd "${TMP_DIR}/project/subdir"
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --gdb-port 32331 \
        --rtt-port 39021 \
        --stop \
        > "${STOP_OUT}" 2>&1
) || fail "--stop failed."

grep -Fq 'Stop signal sent' "${STOP_OUT}" || fail "--stop did not report success."

# Verify the fake server was killed (timeout in case process lingers).
wait_sec=10
while kill -0 "${FAKE_JLINK_PID}" 2>/dev/null; do
    sleep 0.5
    ((wait_sec--))
    if ((wait_sec <= 0)); then
        fail "--stop should have killed JLinkGDBServer (PID ${FAKE_JLINK_PID})."
    fi
done

# --- --stop on idle session (no matching process) should exit 1 ---
log_info "Test 12: Stop command on idle session fails..."
pkill -f "JLinkGDBServer.*-port 32331.*-RTTTelnetPort 39021" 2>/dev/null || true

# Test PID file cleanup
project_temp_dir="$(ls -td /tmp/jlink-rtt-project-* 2>/dev/null | head -n 1)"
if [[ -n "${project_temp_dir}" ]]; then
    touch "${project_temp_dir}/jlink_rtt.pid"
fi
(
    cd "${TMP_DIR}/project/subdir"
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --gdb-port 32331 \
        --rtt-port 39021 \
        --stop \
        > "${STOP_OUT}" 2>&1
) && fail "--stop on idle session should exit 1." || true

grep -Fq 'No running RTT session' "${STOP_OUT}" || fail "--stop should report no session."

if [[ -n "${project_temp_dir}" && -f "${project_temp_dir}/jlink_rtt.pid" ]]; then
    fail "--stop did not clean up stale PID file."
fi

# --- --send on idle session should exit 1 ---
log_info "Test 13: Send command on idle session fails..."
SEND_IDLE_OUT="${TMP_DIR}/send_idle.log"
(
    cd "${TMP_DIR}/project/subdir"
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --send "test_cmd" \
        > "${SEND_IDLE_OUT}" 2>&1
) && fail "--send on idle session should exit 1." || true

grep -Fq 'No running RTT session found' "${SEND_IDLE_OUT}" || fail "--send did not report missing session."

# --- --send injected into running session ---
log_info "Test 14: Send command injected into running RTT session..."
pkill -f "python3.*simulate_ports" 2>/dev/null || true
pkill -f "JLinkGDBServer.*-port 32331.*-RTTTelnetPort 39021" 2>/dev/null || true
rm -f "${TMP_DIR}/downlink_received.log"

# Start jlink-rtt in background
RTT_STREAM_LOG="${TMP_DIR}/rtt_stream.log"
(
    cd "${TMP_DIR}/project/subdir"
    JLINK_RTT_TEST_TMP="${TMP_DIR}" \
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --no-reset \
        --no-resume \
        > "${RTT_STREAM_LOG}" 2>&1
) &
RTT_CLIENT_PID=$!

# Wait for control port file to appear (up to 3s)
project_temp_dir="$(ls -td /tmp/jlink-rtt-project-* 2>/dev/null | head -n 1)"
for _ in $(seq 1 30); do
    [[ -n "${project_temp_dir}" && -f "${project_temp_dir}/rtt_ctrl.port" ]] && break
    sleep 0.1
done
[[ -n "${project_temp_dir}" && -f "${project_temp_dir}/rtt_ctrl.port" ]] || fail "rtt_ctrl.port was not created."

# Send plain text command
SEND_OUT="${TMP_DIR}/send_result.log"
(
    cd "${TMP_DIR}/project/subdir"
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --send "bms_test_charge" \
        > "${SEND_OUT}" 2>&1
) || fail "--send failed to inject command."

grep -Fq 'Downlink command sent' "${SEND_OUT}" || fail "--send did not report success."

# Send hex command
(
    cd "${TMP_DIR}/project/subdir"
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --send "0102030a" \
        --hex \
        >> "${SEND_OUT}" 2>&1
) || fail "--send --hex failed to inject command."

# Send hex command with multiple 0x prefixes
(
    cd "${TMP_DIR}/project/subdir"
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --send "0x0a 0x0b" \
        --hex \
        >> "${SEND_OUT}" 2>&1
) || fail "--send --hex with 0x prefixes failed to inject command."

# Stop session cleanly
(
    cd "${TMP_DIR}/project/subdir"
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --stop \
        > /dev/null 2>&1
) || true

wait "${RTT_CLIENT_PID}" 2>/dev/null || true

# Verify downlink received data contains both commands
grep -Fq 'bms_test_charge' "${TMP_DIR}/downlink_received.log" || fail "Downlink did not receive bms_test_charge."
if ! od -An -tx1 "${TMP_DIR}/downlink_received.log" | grep -q '01 02 03 0a'; then
    fail "Downlink did not receive hex sequence 01 02 03 0a."
fi
if ! od -An -tx1 "${TMP_DIR}/downlink_received.log" | grep -q '0a 0b'; then
    fail "Downlink did not receive hex sequence 0a 0b."
fi

# --- Invalid --out path error handling & clean exit ---
log_info "Test 15: Invalid --out path fails fast without leaking control port..."
pkill -f "python3.*simulate_ports" 2>/dev/null || true
pkill -f "JLinkGDBServer.*-port 32331" 2>/dev/null || true
sleep 0.2
INVALID_OUT_LOG="${TMP_DIR}/invalid_out.log"
(
    cd "${TMP_DIR}/project/subdir"
    JLINK_RTT_TEST_TMP="${TMP_DIR}" \
    PATH="${TMP_DIR}/bin:${PATH}" \
    "${BINARY_PATH}" \
        --project-root "${TMP_DIR}/project" \
        --out "/nonexistent_dir_cannot_create/rtt.log" \
        --no-reset \
        --no-resume \
        > "${INVALID_OUT_LOG}" 2>&1
) && fail "Invalid --out path should exit non-zero." || true

grep -Fq 'Failed to open RTT output file' "${INVALID_OUT_LOG}" || fail "Did not report open file error."
if [[ -n "${project_temp_dir}" && -f "${project_temp_dir}/rtt_ctrl.port" ]]; then
    fail "rtt_ctrl.port leaked after failed --out opening."
fi

log_info "All jlink-rtt automated coverage tests passed successfully!"
