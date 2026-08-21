#!/usr/bin/env python3
"""stock_tools — yomi wire extension 狗食：注册 `stock_quote` 工具。

用法：
    python3 stock_tools.py            # 连默认 daemon socket
    YOMI_SOCKET=unix:///tmp/yomi-daemon-test.sock python3 stock_tools.py

返回确定性的伪实时报价（演示协议全链路，不接真实行情）。
"""

import hashlib
import time

from yomi_ext import Ext

ext = Ext()
ext.tool(
    "stock_quote",
    "查询股票伪实时报价（演示用，确定性伪数据）",
    {
        "type": "object",
        "properties": {
            "symbol": {"type": "string", "description": "股票代码，如 600519"},
        },
        "required": ["symbol"],
    },
    level="safe",
)


@ext.on("stock_quote")
def quote(args: dict) -> dict:
    symbol = str(args.get("symbol", "")).strip()
    if not symbol:
        raise ValueError("symbol is required")
    # 确定性伪报价：同一 symbol 同一分钟同一价格。
    seed = int(hashlib.md5(f"{symbol}:{time.time() // 60}".encode()).hexdigest(), 16)
    price = round(50 + (seed % 10_000) / 100.0, 2)
    change = round(((seed >> 16) % 800 - 400) / 100.0, 2)
    return {"symbol": symbol, "price": price, "change_pct": change, "demo": True}


if __name__ == "__main__":
    print(f"[stock_tools] registered, serving… (socket: {ext.sock.getpeername()!r})")
    ext.serve_forever()
