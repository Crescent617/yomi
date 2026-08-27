# Writing a yomi Extension

An extension is an **external process that registers custom tools into the
agent over the wire protocol** (unix-socket IPC). No plugin loader, no
lifecycle API: connect, register, serve tool calls — everything vanishes
when you disconnect (RAII).

Design details (optional): `docs/archive/extension-phase1.md`.

## Quick start (Python)

Copy `examples/yomi_ext.py` next to your script, then:

```python
from yomi_ext import Ext

ext = Ext()  # connects to the daemon socket, does the hello handshake

ext.tool(
    "stock_quote",            # rules: start with a letter, only [a-zA-Z0-9_-], NO dots
    "查询股票伪实时报价（演示）",  # shown to the model verbatim
    {"type": "object",
     "properties": {"symbol": {"type": "string"}},
     "required": ["symbol"]},
    level="safe",             # safe | caution | dangerous; default: caution (needs user approval)
)

@ext.on("stock_quote")
def quote(args):
    return {"price": 81.08, "demo": True}

ext.serve_forever()  # pull → execute → result; exits when the daemon drops the connection
```

Run `python3 stock_tools.py`. From then on, **new agent sessions** have the
tool (running agents get it after `/clear` or ~5 min idle respawn).
Kill the process → tool gone, in-flight calls error
`extension tool provider disconnected`.

## Rules that will bite you

- **Naming**: letter first, `[a-zA-Z0-9_-]` only. `stock.quote` is rejected
  (OpenAI 400). Same name twice → second registration rejected.
  Builtin name collision → builtin wins, your tool never appears.
- **Timeouts**: a call must be answered within **60 s** (fixed); `ext_pull`
  returns `null` every **55 s** idle — just pull again.
- **Single worker**: one pending `ext_pull` per registration; a second
  concurrent pull is an error. Parallelism = more processes.
- **Teardown is RAII only**: no `unregister` — close the connection / exit
  the process. Daemon restart = your read fails; restart your process to
  re-register.
- **Don't pipeline on one connection**: a pending `ext_pull` holds your
  connection's request loop — an `ext_result` sent behind it on the same
  connection can wait up to 55 s and hit the 60 s call cap. Use the SDK's
  sequential loop, or a second connection.
- **Permissions**: `tool_blocklist` regexes in daemon config apply to your
  tools too. `level` defaults to `caution` (approval card per call).
- **Auth**: the unix socket file *is* the credential. Default path
  `~/Library/Application Support/yomi/daemon.sock`, override `YOMI_SOCKET=unix://…`.

## Supervised mode (optional)

Daemon `config.toml`:

```toml
[[extensions]]
name = "stock-tools"
command = ["python3", "/abs/path/stock_tools.py"]
```

Listed = spawned at daemon boot (own process group), daemon death =
SIGTERM the group, crash = respawn after 5 s. Logs:
`<data_dir>/logs/ext-<name>.log`.

## Source routing (`ext_route`, for message producers)

```json
→ {"ext_route": {"source": "gitlab-ci", "key": "proj123/pipelines"}}
← {"session_id": "sess_…", "created": false}
```

One stable session per `source`+`key` (created once, reused). Then call the
existing `send_message` RPC with text prefixed `[From source:<name>] `.

## Wire protocol (only if you skip the SDK)

- Unix socket, frames = **4-byte big-endian length + JSON**, both directions.
- `{"type":"request","id":1,"method":<M>}` →
  `{"type":"response","id":1,"body":{"status":"ok","result":…}}` or
  `{"status":"err","error":{code,message}}`.
- First call `{"method":"hello"}` → `{"proto":28,"instance_id":"…"}`.
- Methods: `ext_register {kind:"tool",name,desc,schema,level?}` → `{registration}`;
  `ext_pull {registration}` → `{call_id,name,args}` or `null`;
  `ext_result {call_id,output,is_error}` → `null`;
  `ext_route {source,key}` → `{session_id,created}`.

## Verify it works

Daemon log `extension tool registered … tool=<name>` → ask a **new**
session to use the tool → kill your process → daemon log
`extension tool swept`, fresh session says the tool doesn't exist.
Daemon log: `~/.yomi/logs/daemon.<date>.log`.
