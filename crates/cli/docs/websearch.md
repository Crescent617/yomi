# web_search 工具

内建 `web_search`（feature `websearch` 默认开；`WEBSEARCH_TOOL_NAME` 不门控，权限解析对扩展提供的同名工具仍生效）。引擎按优先级**串行 fallback**，第一个返回非空结果的胜出；全失败时报错拼接各引擎原因（首要诊断入口）。

## 引擎与启用

启用 = 设对应 env，优先级从高到低：

| 优先级 | 引擎 | env | 备注 |
| --- | --- | --- | --- |
| 1 | SearXNG | `SEARXNG_URL` | 自托管，推荐 |
| 2 | Kimi | `KIMI_AGENT_API_KEY` | 可选 `KIMI_SEARCH_ENDPOINT` 换端点 |
| 3 | Serper | `SERPER_API_KEY` | 付费 Google Search API |
| 4 | Brave | `BRAVE_API_KEY` | 付费 Brave Search API |
| 5 | DuckDuckGo | — | 免费 HTML 抓取兜底 |
| 6 | Bing | — | 免费 HTML 抓取兜底 |

## 配置路径

- daemon/CLI：`yomi config set env.SEARXNG_URL http://127.0.0.1:8080`——落 config.toml `[env]` 段，启动时注入进程环境且**覆盖宿主同名变量**；改后 `daemon restart` 生效（重启子进程会先清掉旧注入值再重读 `[env]`，删条目也须重启才真失效）。
- GUI：启动时读 `~/.env`（Windows 为 `%USERPROFILE%\.env`），改后重启 GUI。

## 坑

- SearXNG：yomi 请求 `format=json`，而 SearXNG 默认只开 HTML——`settings.yml` 须加 `formats: [html, json]` 并重启容器，否则该引擎静默失败、落到下一个。docker compose 搭建见仓库 `README.md`「Web Search」节。
- `fetch_content=true`（默认）只抓 top 3 正文、每页截 5K。结果每行带 `Source: <引擎名>`，可据此确认实际命中哪个引擎。
- 抓完整网页走 shell curl 落盘——内建 `web_fetch` 已移除，理由见仓库 `docs/design/webfetch-removal.md`。

## 冒烟测试

`yomi run --yolo "用 web_search 查 <词>，报告结果来源引擎"`
