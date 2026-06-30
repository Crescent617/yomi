#!/usr/bin/env python3
"""
yomi-kernel IPC client in Python.

Connects to the yomi kernel daemon over a Unix socket and speaks
length-prefixed JSON frames (the Wire protocol).

Usage examples:
    python3 yomi_client.py hello
    python3 yomi_client.py list_sessions
    python3 yomi_client.py create_session
    python3 yomi_client.py send_message <session_id> "hello world"
    python3 yomi_client.py get_session_messages <session_id>
"""
import argparse
import json
import os
import socket
import struct
import sys

MAX_FRAME_SIZE = 8 * 1024 * 1024
WIRE_PROTOCOL_VERSION = 5


def socket_path() -> str:
    """Resolve the daemon socket path the same way the kernel does."""
    env = os.environ.get("YOMI_SOCKET")
    if env:
        return env

    xdg = os.environ.get("XDG_RUNTIME_DIR")
    if xdg:
        return os.path.join(xdg, "yomi", "daemon.sock")

    # macOS: ~/Library/Application Support/yomi/daemon.sock
    home = os.path.expanduser("~")
    mac_path = os.path.join(home, "Library", "Application Support", "yomi", "daemon.sock")
    if os.path.exists(mac_path):
        return mac_path

    # Linux fallback: ~/.local/share/yomi/daemon.sock
    linux_path = os.path.join(home, ".local", "share", "yomi", "daemon.sock")
    if os.path.exists(linux_path):
        return linux_path

    # Final fallback
    fallback = "/tmp/yomi-daemon.sock"
    if os.path.exists(fallback):
        return fallback

    raise FileNotFoundError("Cannot find yomi daemon socket. Is the daemon running?")


class Client:
    def __init__(self, path: str):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(path)
        self._next_id = 1

    def _next_req_id(self) -> int:
        rid = self._next_id
        self._next_id += 1
        return rid

    def _send_frame(self, obj: dict):
        payload = json.dumps(obj, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
        if len(payload) > MAX_FRAME_SIZE:
            raise ValueError(f"frame too large: {len(payload)} > {MAX_FRAME_SIZE}")
        self.sock.sendall(struct.pack(">I", len(payload)) + payload)

    def _recv_frame(self) -> dict:
        len_bytes = self._recv_exact(4)
        length = struct.unpack(">I", len_bytes)[0]
        if length > MAX_FRAME_SIZE:
            raise ValueError(f"frame too large: {length} > {MAX_FRAME_SIZE}")
        payload = self._recv_exact(length)
        return json.loads(payload.decode("utf-8"))

    def _recv_exact(self, n: int) -> bytes:
        buf = b""
        while len(buf) < n:
            chunk = self.sock.recv(n - len(buf))
            if not chunk:
                raise ConnectionResetError("daemon closed connection")
            buf += chunk
        return buf

    def request(self, method: str, **kwargs) -> dict:
        """Send a request and wait for the matching Response."""
        req_id = self._next_req_id()
        msg = {"type": "request", "id": req_id, "method": {method: kwargs}}
        self._send_frame(msg)

        while True:
            frame = self._recv_frame()
            if frame.get("type") == "response" and frame.get("id") == req_id:
                return frame
            if frame.get("type") == "ping":
                self._send_frame({"type": "pong"})
            # Events and responses for other requests are dropped here.

    def hello(self) -> dict:
        """Handshake: verify wire protocol version."""
        return self.request("hello")

    def list_projects(self) -> dict:
        return self.request("list_projects")

    def create_project(self, dir: str, name: str | None = None) -> dict:
        return self.request("create_project", dir=dir, name=name)

    def get_project(self, project_id: str) -> dict:
        return self.request("get_project", project_id=project_id)

    def create_session(self, project_id: str | None = None, working_dir: str | None = None) -> dict:
        return self.request("create_session", project_id=project_id, working_dir=working_dir, auto_approve_level="safe")

    def list_sessions(self, project_id: str | None = None, limit: int = 50) -> dict:
        return self.request("list_sessions", project_id=project_id, before=None, limit=limit)

    def get_session_messages(self, session_id: str) -> dict:
        return self.request("get_session_messages", session_id=session_id)

    def get_session_status(self, session_id: str) -> dict:
        return self.request("get_session_status", session_id=session_id)

    def send_message(self, session_id: str, text: str) -> dict:
        blocks = [{"type": "text", "text": text}]
        return self.request("send_message", session_id=session_id, blocks=blocks)

    def subscribe(self, session_id: str):
        """Subscribe to session events. Returns the request response; events must be read separately."""
        return self.request("subscribe", session_id=session_id)

    def close(self):
        self.sock.close()


def pretty_print(obj: dict):
    print(json.dumps(obj, indent=2, ensure_ascii=False))


def main():
    parser = argparse.ArgumentParser(description="Yomi kernel IPC client")
    parser.add_argument("command", choices=[
        "hello", "list_projects", "create_project", "get_project",
        "create_session", "list_sessions", "get_session_messages",
        "get_session_status", "send_message", "subscribe", "interactive"
    ])
    parser.add_argument("args", nargs="*", help="positional arguments for the command")
    parser.add_argument("--socket", "-s", help="override daemon socket path")
    args = parser.parse_args()

    path = args.socket or socket_path()
    client = Client(path)

    try:
        if args.command == "hello":
            pretty_print(client.hello())

        elif args.command == "list_projects":
            pretty_print(client.list_projects())

        elif args.command == "create_project":
            if len(args.args) < 1:
                print("Usage: create_project <dir> [name]", file=sys.stderr)
                sys.exit(1)
            pretty_print(client.create_project(dir=args.args[0], name=args.args[1] if len(args.args) > 1 else None))

        elif args.command == "get_project":
            if not args.args:
                print("Usage: get_project <project_id>", file=sys.stderr)
                sys.exit(1)
            pretty_print(client.get_project(args.args[0]))

        elif args.command == "create_session":
            pretty_print(client.create_session(project_id=args.args[0] if args.args else None))

        elif args.command == "list_sessions":
            limit = int(args.args[0]) if args.args else 50
            pretty_print(client.list_sessions(limit=limit))

        elif args.command == "get_session_messages":
            if not args.args:
                print("Usage: get_session_messages <session_id>", file=sys.stderr)
                sys.exit(1)
            pretty_print(client.get_session_messages(args.args[0]))

        elif args.command == "get_session_status":
            if not args.args:
                print("Usage: get_session_status <session_id>", file=sys.stderr)
                sys.exit(1)
            pretty_print(client.get_session_status(args.args[0]))

        elif args.command == "send_message":
            if len(args.args) < 2:
                print("Usage: send_message <session_id> <text>", file=sys.stderr)
                sys.exit(1)
            pretty_print(client.send_message(args.args[0], " ".join(args.args[1:])))

        elif args.command == "subscribe":
            if not args.args:
                print("Usage: subscribe <session_id>", file=sys.stderr)
                sys.exit(1)
            pretty_print(client.subscribe(args.args[0]))
            print("\n-- Listening for events (Ctrl-C to stop) --")
            try:
                while True:
                    frame = client._recv_frame()
                    pretty_print(frame)
            except KeyboardInterrupt:
                pass

        elif args.command == "interactive":
            # Simple REPL
            print("Yomi IPC client. Type 'help' for commands, 'quit' to exit.")
            while True:
                try:
                    line = input("> ").strip()
                except (EOFError, KeyboardInterrupt):
                    break
                if not line or line == "quit":
                    break
                if line == "help":
                    print("Commands: hello, list_projects, list_sessions, create_session, send_message <sid> <text>, ...")
                    continue
                parts = line.split()
                cmd = parts[0]
                if cmd == "hello":
                    pretty_print(client.hello())
                elif cmd == "list_projects":
                    pretty_print(client.list_projects())
                elif cmd == "list_sessions":
                    pretty_print(client.list_sessions())
                elif cmd == "create_session":
                    pretty_print(client.create_session(project_id=parts[1] if len(parts) > 1 else None))
                elif cmd == "send_message" and len(parts) >= 3:
                    pretty_print(client.send_message(parts[1], " ".join(parts[2:])))
                else:
                    print(f"Unknown command: {cmd}")
    finally:
        client.close()


if __name__ == "__main__":
    main()
