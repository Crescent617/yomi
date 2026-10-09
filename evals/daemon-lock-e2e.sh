#!/bin/bash
# Daemon 单例锁 e2e：data_dir 为键的互斥 + 环境隔离并行 + restart 交接。
#
# 隔离方式与 ext-e2e.sh 相同：独立 data_dir + YOMI_SOCKET 指向独立
# socket + YOMI_CONFIG 指到临时空 config（不连任何渠道）。
# 从 yomi 会话里跑时进程环境自带注入的 YOMI_DATA_DIR，必须显式覆盖。
set -u
[ "$(uname -s)" = "Darwin" ] || { echo "unix-only e2e (flock)"; exit 2; }
cd "$(dirname "$0")/.."
YOMI="$(pwd)/target/debug/yomi"
[ -x "$YOMI" ] || { echo "build CLI first: cargo build -p cli"; exit 2; }

E2E="$(mktemp -d /tmp/yomi-lock-e2e.XXXXXX)"
printf '# e2e isolated config: no channels, no real accounts.\n' > "$E2E/config.toml"
DA="$E2E/data-a"; mkdir -p "$DA"
DC="$E2E/data-c"
# 失败路径也要收走 daemon：先停 A/C 再清目录。
cleanup() {
  env YOMI_DATA_DIR="$DA" YOMI_CONFIG="$E2E/config.toml" YOMI_SOCKET="unix://$E2E/a.sock"     "$YOMI" daemon stop >/dev/null 2>&1 || true
  env YOMI_DATA_DIR="$DA" YOMI_CONFIG="$E2E/config.toml" YOMI_SOCKET="unix://$E2E/d.sock"     "$YOMI" daemon stop >/dev/null 2>&1 || true
  [ -d "$DC" ] && env YOMI_DATA_DIR="$DC" YOMI_CONFIG="$E2E/config.toml" YOMI_SOCKET="unix://$E2E/c.sock"     "$YOMI" daemon stop >/dev/null 2>&1 || true
  rm -rf "$E2E"
}
trap cleanup EXIT

# 同一 data_dir、两个不同 socket：第二个 daemon 必须被锁拒绝。
SB="$E2E/b.sock"

run_daemon() { # data_dir socket log
  env YOMI_DATA_DIR="$1" YOMI_CONFIG="$E2E/config.toml" YOMI_SOCKET="unix://$2" \
    "$YOMI" daemon start >>"$3" 2>&1 &
  echo $!
}
wait_up() { # data_dir socket
  # 就绪信号用 cron list 的 wire 往返（daemon 先 bind 后初始化存储，
  # status 通 ≠ 可用）。
  for _ in $(seq 1 50); do
    env YOMI_DATA_DIR="$1" YOMI_CONFIG="$E2E/config.toml" YOMI_SOCKET="unix://$2" \
      "$YOMI" cron list >/dev/null 2>&1 && return 0
    sleep 0.2
  done
  return 1
}
wait_down() { # socket
  for _ in $(seq 1 50); do
    env YOMI_SOCKET="unix://$1" "$YOMI" daemon status 2>/dev/null | grep -q "not running" && return 0
    sleep 0.2
  done
  return 1
}
# 锁与 meta 在 <data_dir>/run/ 下（2026-10-09 起），路径确定，直读。
lock_meta() { # data_dir
  echo "$1/run/daemon.lock.meta"
}

echo "── 1. daemon A 起（data-a / a.sock）"
run_daemon "$DA" "$E2E/a.sock" "$E2E/a.log"
wait_up "$DA" "$E2E/a.sock" || { echo "FAIL: A did not start"; cat "$E2E/a.log"; exit 1; }
echo "ok"

echo "── 2. daemon B 同 data_dir 不同 socket：必须被锁拒绝"
B_OUT="$(env YOMI_DATA_DIR="$DA" YOMI_CONFIG="$E2E/config.toml" YOMI_SOCKET="unix://$SB" \
  "$YOMI" daemon start 2>&1)" && { echo "FAIL: B started (lock not enforced). $B_OUT"; exit 1; }
echo "$B_OUT" | grep -q "already owned by another yomi daemon" \
  || { echo "FAIL: B error lacks lock message: $B_OUT"; exit 1; }
echo "$B_OUT" | grep -q "daemon stop" \
  || { echo "FAIL: B error lacks remediation hint: $B_OUT"; exit 1; }
[ -S "$SB" ] && { echo "FAIL: B left a socket file behind"; exit 1; }
echo "ok ($B_OUT)"

echo "── 3. daemon C 不同 data_dir：并行不受影响"
mkdir -p "$DC"
run_daemon "$DC" "$E2E/c.sock" "$E2E/c.log"
wait_up "$DC" "$E2E/c.sock" || { echo "FAIL: C did not start"; cat "$E2E/c.log"; exit 1; }
echo "ok"

echo "── 4. restart A：锁交接后新 daemon 持锁"
env YOMI_DATA_DIR="$DA" YOMI_CONFIG="$E2E/config.toml" YOMI_SOCKET="unix://$E2E/a.sock" \
  "$YOMI" daemon restart >/dev/null 2>&1 || { echo "FAIL: restart A"; cat "$E2E/a.log"; exit 1; }
wait_up "$DA" "$E2E/a.sock" || { echo "FAIL: A not up after restart"; exit 1; }
META_AFTER="$(grep '"pid"' "$(lock_meta "$DA")" | grep -o '[0-9]*' | head -1)"
[ -n "$META_AFTER" ] || { echo "FAIL: no meta after restart"; exit 1; }
# 拒绝方报错里的 pid 与当前持有者一致（再次起 B 验证）
B_OUT2="$(env YOMI_DATA_DIR="$DA" YOMI_CONFIG="$E2E/config.toml" YOMI_SOCKET="unix://$SB" \
  "$YOMI" daemon start 2>&1 || true)"
echo "$B_OUT2" | grep -q "$META_AFTER" \
  || { echo "FAIL: contender report pid $META_AFTER not in: $B_OUT2"; exit 1; }
echo "ok (holder pid $META_AFTER)"

echo "── 5. kill -9 持有者：无 stale 锁，竞争者立即获锁"
A_PID="$(grep '"pid"' "$(lock_meta "$DA")" | grep -o '[0-9]*' | head -1)"
kill -9 "$A_PID" 2>/dev/null || { echo "FAIL: kill A pid $A_PID"; exit 1; }
sleep 0.5
run_daemon "$DA" "$E2E/d.sock" "$E2E/d.log"
wait_up "$DA" "$E2E/d.sock" || { echo "FAIL: D did not acquire lock after kill -9"; cat "$E2E/d.log"; exit 1; }
D_PID="$(grep '"pid"' "$(lock_meta "$DA")" | grep -o '[0-9]*' | head -1)"
[ "$D_PID" != "$A_PID" ] || { echo "FAIL: meta pid unchanged after kill -9 recovery"; exit 1; }
echo "ok (new holder pid $D_PID)"

echo "── 6. 收尾：D、C 停"
env YOMI_DATA_DIR="$DA" YOMI_CONFIG="$E2E/config.toml" YOMI_SOCKET="unix://$E2E/d.sock" "$YOMI" daemon stop >/dev/null 2>&1
env YOMI_DATA_DIR="$DC" YOMI_CONFIG="$E2E/config.toml" YOMI_SOCKET="unix://$E2E/c.sock" "$YOMI" daemon stop >/dev/null 2>&1
wait_down "$E2E/d.sock" && wait_down "$E2E/c.sock" || { echo "FAIL: cleanup"; exit 1; }
echo "ok"

echo "── 7. K8s pod 重建场景：残留 pid 文件指向存活但无关的进程，不得挡启动"
# pid 文件是 socket 同级的 .pid；pod 重建后新 pid namespace 里，旧 pod
# 留下的 pid 号极易被无关进程占用——若拿它当活性判据会误拒启动。
# 用本脚本自己的 pid（必活、且不是 daemon）种残留。
echo $$ > "$E2E/d.pid"
run_daemon "$DA" "$E2E/d.sock" "$E2E/e.log"
wait_up "$DA" "$E2E/d.sock" || { echo "FAIL: stale-pid/live-process blocked start"; cat "$E2E/e.log"; exit 1; }
env YOMI_DATA_DIR="$DA" YOMI_CONFIG="$E2E/config.toml" YOMI_SOCKET="unix://$E2E/d.sock" "$YOMI" daemon stop >/dev/null 2>&1
wait_down "$E2E/d.sock" || { echo "FAIL: cleanup after stale-pid test"; exit 1; }
echo "ok"

echo "PASS: daemon-lock e2e"
