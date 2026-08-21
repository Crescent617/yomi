# Writing a yomi Extension

A yomi extension is an **external process that registers custom tools into
the agent over the wire protocol** (unix-socket IPC). There is no plugin
loader, no SDK requirement, no lifecycle API: connect, register, serve
tool calls, and everything you registered disappears when you disconnect
(RAII).

This document is the complete authoring reference. It is written to be
machine-readable: every JSON shape below is exact.

- Phase-1 surface: **custom tools** (`ext_register` / `ext_pull` /
  `ext_result`) and **source routing** (`ext_route`).
- Not in phase 1: gate hooks, unregister RPC, persistence of registrations.
- Design rationale (optional reading): `docs/design/extension-phase1.md`.

## 1. Quick start (Python, ~30 lines)

The repo ships a minimal SDK: `examples/yomi_ext.py`. Copy it next to your
script (or import it from the repo path).

```python
from yomi_ext import Ext

ext = Ext()  # connects to the daemon socket and performs the hello handshake

ext.tool(
    "stock_quote",                       # see §5 naming rules
    "查询股票伪实时报价（演示）",           # desc: shown to the model verbatim
    {                                    # JSON schema for the arguments
        "type": "object",
        "properties": {"symbol": {"type": "string"}},
        "required": ["symbol"],
    },
    level="safe",                        # permission level, see §6
)

@ext.on("stock_quote")
def quote(args):
    return {"price": 81.08, "demo": True}

ext.serve_forever()  # pull → execute → result loop; exits when the daemon drops the connection
```

Run it: `python3 stock_tools.py`. From this point, **every newly spawned
agent** has `stock_quote` in its tool table. Agents already running pick it
up on their next respawn (agents exit after ~5 min idle, or `/clear`).

Kill the process → the tool immediately vanishes from future spawns, and
any in-flight call fails with `extension tool provider disconnected`.

## 2. Wire protocol basics (SDK-free authors)

### Transport

- Unix domain socket. Default path: `~/Library/Application Support/yomi/daemon.sock`;
  override via `YOMI_SOCKET=unix://<path>`.
- Frames: **4-byte big-endian length prefix + JSON payload**, both directions.

### Message envelope

```json
→ {"type": "request",  "id": 1, "method": <ReqMethod>}
← {"type": "response", "id": 1, "body": {"status": "ok",  "result": ...}}
← {"type": "response", "id": 1, "body": {"status": "err", "error": {"code": "...", "message": "...", "detail": ...}}}
```

The daemon may also send `{"type": "event"|"noti"|"ping"}` frames — ignore
unless subscribed; answer `ping` with `{"type": "pong"}` if you want to be
polite (the daemon also tolerates silence on this).

### Handshake

First call must be `{"method": "hello"}` → result
`{"proto": 28, "instance_id": "…"}`. `proto` is the daemon's wire protocol
version; extensions should just record it (breaking changes bump it).

## 3. The tool lifecycle (exact RPC shapes)

### 3.1 `ext_register` — register one tool

```json
→ {"ext_register": {
     "kind": "tool",
     "name": "stock_quote",
     "desc": "查询股票伪实时报价（演示）",
     "schema": {"type": "object", "properties": {"symbol": {"type": "string"}}, "required": ["symbol"]},
     "level": "safe"}}
← {"registration": "ext_01j..."}
```

- `kind`: only `"tool"` in phase 1 (anything else → `ext_bad_kind`).
- `level`: `"safe" | "caution" | "dangerous"`; default `caution`.
- `desc` and `schema` go into the model's tool list **verbatim** — write
  them as carefully as for a builtin tool.
- Errors: duplicate name in the extension namespace →
  `ext_register_failed` ("already registered"); invalid name → see §5.

### 3.2 `ext_pull` — long-poll for work (55 s heartbeat)

```json
→ {"ext_pull": {"registration": "ext_01j..."}}
← {"call_id": "c_91a", "name": "stock_quote", "args": {"symbol": "600519"}}
← null                       // 55 s elapsed with no work; pull again immediately
```

- Exactly-once by construction: one work item is popped by exactly one pull.
- **Single-worker rule**: only one pending pull per registration; a second
  concurrent pull is rejected. For parallelism, run more processes.
- `registration` belongs to your connection — pulling someone else's is an error.

### 3.3 `ext_result` — deliver the outcome

```json
→ {"ext_result": {"call_id": "c_91a", "output": "1900.00", "is_error": false}}
← null
```

- `output` is a **string** (JSON-encode structured data yourself).
- `is_error: true` marks the tool result as an error to the model.
- The caller times out after **60 s**; results arriving later are discarded
  silently (recorded in daemon logs). Wrong-connection `call_id` → error.

### 3.4 Teardown — there is only RAII

No `unregister` call exists. To remove a tool: **close the connection**
(or exit the process). The daemon sweeps everything owned by the
connection instantly:

- the tool disappears from future agent spawns,
- in-flight and queued work items fail with `extension tool provider disconnected`,
- the name becomes re-registerable immediately.

The daemon restarting has the same effect on your side: your read fails →
exit and restart your process to re-register.

## 4. Supervised mode: let the daemon babysit your process

Instead of starting the extension yourself, declare it in the daemon's
`config.toml`:

```toml
[[extensions]]
name = "stock-tools"
command = ["python3", "/abs/path/to/stock_tools.py"]
```

Semantics (no knobs):

1. **listed = spawned** at daemon boot (own process group),
2. **daemon dies → whole process group SIGTERM** (release/restart just works),
3. **crash → fixed 5 s backoff respawn** (no restart policy options).

Logs: `<data_dir>/logs/ext-<name>.log`. Supervised and self-started
extensions share the identical registration contract.

## 5. Tool naming rules (hard constraint)

Provider APIs differ; the strictest common denominator (OpenAI) applies:

- must **start with a letter**,
- may contain only **`[a-zA-Z0-9_-]`** — **no dots** (`stock.quote` is
  rejected by OpenAI with a 400; validated at registration).

Name conflicts:

| Conflict | Outcome |
|---|---|
| extension vs extension (same name) | second registration is rejected; first registrant owns the name until it disconnects |
| extension vs builtin tool | builtin wins; the extension registers "fine" but is skipped at every spawn (warned in logs) |
| matches `tool_blocklist` in config | blocked at spawn, same as builtins |

Overriding builtins is deliberately not supported (see design doc).

## 6. Permission levels & approval

- `safe`: executes immediately, no approval card.
- `caution` (default): the user must approve each call (channel approval
  card / GUI prompt), same as builtin tools of that level.
- `dangerous`: reserved; treated as highest-risk.

`tool_blocklist` regexes in the daemon config apply to extension tools
exactly as to builtins.

## 7. Source routing (`ext_route`)

For processes that *produce* messages (webhook bridges, pollers):

```json
→ {"ext_route": {"source": "gitlab-ci", "key": "proj123/pipelines"}}
← {"session_id": "sess_…", "created": false}
```

- `source` + `key` map to one stable session (created on first call,
  reused after; survives your restarts). Use distinct keys per topic.
- Then post via the existing `send_message` RPC, prefixing the text with
  `[From source:<name>] ` so the agent can tell events from humans.
- Reply delivery: point your messages at a session that is already bound
  to a chat channel (look the session id up once and skip `ext_route`), or
  have the agent forward results (e.g., via the lark skill).

## 8. Dogfooding checklist (verify your extension)

1. Daemon log shows `extension tool registered registration=… tool=<name>`.
2. Ask the agent (a **new** session, or `/clear`) to use the tool → the
   model emits the tool call, your `ext_pull` receives it, your handler
   runs, the answer composes normally.
3. Kill your process → daemon logs `extension tool swept`; a fresh session
   reports the tool does not exist; a raced call fails with
   `extension tool provider disconnected`.
4. Restart the daemon → your process exits (or is group-killed if
   supervised); re-run/re-spawn → it re-registers under a new id.

Logs to watch: daemon `~/.yomi/logs/daemon.<date>.log`, supervised
extensions `<data_dir>/logs/ext-<name>.log`.

## 9. FAQ

- **Tool doesn't show up**: agents snapshot tools at spawn. `/clear` the
  session or wait for the ~5 min idle respawn. Also check §5 conflicts.
- **`function name is invalid` (OpenAI 400)**: dotted name, see §5.
- **Call hangs 60 s then errors**: your handler didn't answer in time; the
  60 s cap is fixed (long jobs: return early with "started", poll later).
- **"a pull is already pending"**: single-worker rule (§3.2) — you pulled
  twice concurrently from one registration.
- **Auth**: the unix socket *is* the credential (file permissions). Keep
  the socket path private; remote TCP does not exist yet.
