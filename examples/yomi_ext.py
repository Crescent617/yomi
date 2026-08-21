"""yomi_ext — yomi wire 外部扩展的 Python SDK（一期：custom tool）。

协议：unix socket + 长度前缀 JSON 帧（4B BE length + JSON）。
流程：Hello 握手 → ext_register 登记工具 → serve_forever 循环
（ext_pull 领单 → 执行 → ext_result 交付）。断开连接即下线（RAII），
daemon 重启后重跑本脚本重注册即可。

用法见 examples/stock_tools.py。
"""

from __future__ import annotations

import json
import os
import socket
import struct
import threading
from typing import Any, Callable

DEFAULT_SOCKET = os.path.expanduser(
    "~/Library/Application Support/yomi/daemon.sock"
)
if "YOMI_SOCKET" in os.environ:
    DEFAULT_SOCKET = os.environ["YOMI_SOCKET"].removeprefix("unix://")


class ExtError(Exception):
    pass


def _send_frame(sock: socket.socket, msg: dict) -> None:
    payload = json.dumps(msg).encode()
    sock.sendall(struct.pack(">I", len(payload)) + payload)


def _recv_frame(sock: socket.socket) -> dict:
    def read_exact(n: int) -> bytes:
        buf = b""
        while len(buf) < n:
            chunk = sock.recv(n - len(buf))
            if not chunk:
                raise ExtError("daemon closed the connection")
            buf += chunk
        return buf

    (length,) = struct.unpack(">I", read_exact(4))
    return json.loads(read_exact(length))


class Ext:
    """一个扩展连接：登记工具并循环领单。"""

    def __init__(self, socket_path: str | None = None):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(socket_path or DEFAULT_SOCKET)
        self._lock = threading.Lock()
        self._next_id = 1
        self._handlers: dict[str, Callable[[dict], Any]] = {}
        self._registration: str | None = None
        self._hello()

    def _call(self, method: Any) -> Any:
        with self._lock:
            req_id = self._next_id
            self._next_id += 1
            _send_frame(self.sock, {"type": "request", "id": req_id, "method": method})
            while True:
                frame = _recv_frame(self.sock)
                if frame.get("type") == "response" and frame.get("id") == req_id:
                    body = frame["body"]
                    status = body.get("status")
                    if status == "err":
                        err = body["error"]
                        raise ExtError(f"{err.get('code')}: {err.get('message')}")
                    return body.get("result")

    def _hello(self) -> None:
        result = self._call("hello")
        self.instance_id = result.get("instance_id")

    def tool(
        self,
        name: str,
        desc: str,
        schema: dict,
        level: str = "caution",
    ) -> str:
        """登记一个 custom tool，返回 registration id。"""
        result = self._call(
            {
                "ext_register": {
                    "kind": "tool",
                    "name": name,
                    "desc": desc,
                    "schema": schema,
                    "level": level,
                }
            }
        )
        self._registration = result["registration"]
        return self._registration

    def on(self, name: str) -> Callable:
        """装饰器：登记工具的处理函数。"""

        def wrap(fn: Callable[[dict], Any]) -> Callable:
            self._handlers[name] = fn
            return fn

        return wrap

    def serve_forever(self) -> None:
        """pull → 执行 → result 循环；daemon 断开即退出（RAII）。"""
        if not self._registration:
            raise ExtError("no tool registered yet (call .tool first)")
        while True:
            work = self._call({"ext_pull": {"registration": self._registration}})
            if work is None:  # 55s 空转心跳
                continue
            call_id, name, args = work["call_id"], work["name"], work["args"]
            handler = self._handlers.get(name)
            try:
                if handler is None:
                    raise ExtError(f"no handler for tool '{name}'")
                result = handler(args)
                output, is_error = (
                    result if isinstance(result, str) else json.dumps(result, ensure_ascii=False)
                ), False
            except Exception as e:  # noqa: BLE001 — 任何执行异常都回给模型
                output, is_error = f"{type(e).__name__}: {e}", True
            self._call({"ext_result": {"call_id": call_id, "output": output, "is_error": is_error}})

    def close(self) -> None:
        self.sock.close()
