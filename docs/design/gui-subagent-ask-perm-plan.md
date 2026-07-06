# GUI Subagent AskUser / Permission 响应 — 执行计划

## 目标

让 GUI 具备与 TUI 相同的能力：当 subagent（`Agent` tool 内部）发起 `ask_user` 或 `permission` 请求时，用户可以在主会话的对应 ToolBlock 内直接响应，无需额外创建侧边栏 session 项或跳转。同时，若用户已跳转到 subagent session，该 session 的独立事件流按常规逻辑渲染全局 PermissionBar/AskUserBar（不做特殊屏蔽），保持简洁。

---

## 范围

### 做（In Scope）
- 后端 `subscribe` 命令自动发现 subagent 并启动后台事件订阅
- 新增 `kernel:subagent_event` 事件通道，仅转发结构事件（过滤高频 delta）
- 前端 `state.svelte.ts` 新增 `SubagentState` 类型与 `handleSubagentEvent` 处理逻辑
- 前端 `+layout.svelte` 监听 `kernel:subagent_event`
- `ToolBlock.svelte` 内联渲染 subagent 的 Permission / AskUser 交互面板
- 响应时正确传递 subagent 的 `session_id` 给 API

### 不做（Out of Scope）
- auto-deny stale request（当前 GUI 主 session 也未实现，后续统一补充）
- 对 subagent session 做特殊渲染屏蔽（全局 PermissionBar/AskUserBar 在 subagent session 中正常显示）
- subagent 历史消息独立分页或特殊展示
- subagent 的 event pump 重连逻辑（首次实现先依赖单次订阅，失败不自动重试）

---

## 数据流设计

```
┌─────────────┐
│  Kernel     │──subagent_session_id──┐
│  (Metadata) │                       │
└──────┬──────┘                       │
       │                              │
       │                              ▼
       │                       ┌──────────────┐
       │                       │ 后台 subscriber │
       │                       │ (tauri::spawn)  │
       │                       └──────┬───────┘
       │                              │
       │  kernel:event                 │  kernel:subagent_event
       │ {session_id, event}          │ {parent_session_id, parent_tool_id, event}
       ▼                              ▼
┌──────────────┐            ┌──────────────┐
│ +layout.svelte│            │ +layout.svelte│
│ handleEvent() │            │ handleSubagentEvent() │
│   → 更新主 session         │   → 找到 parent session   │
│   → 更新 ToolMessage        │   → 更新对应 ToolMessage.subagent │
│      (subagent_session_id)  │      (pending_permission / ask_user)│
└──────────────┘            └──────────────┘
       │                              │
       │                              ▼
       │                       ┌──────────────┐
       │                       │ ToolBlock.svelte │
       │                       │ 内联 Permission/AskUser 面板 │
       │                       └──────────────┘
       ▼
┌──────────────┐
│ 用户点击响应   │
└──────┬───────┘
       │
       ▼
┌──────────────┐
│ api.respondPermission()  │
│ api.respondAskUser()     │
│ 传入 subagent.session_id  │
└──────────────┘
```

---

## 后端改动

### 文件：`crates/gui/src/commands/chat.rs`

在 `subscribe` 命令的 `rx.recv()` 循环中，检测 `ToolEvent::Metadata` 里的 `subagent_session_id`：

1. 提取 `parent_tool_id`（从 metadata 或 fallback 到 `tool_id`）
2. 用 `coordinator.subscribe_session_events(&sub_sid)` 启动后台 subscriber
3. 在后台循环中：
   - 跳过 `ModelEvent::Chunk` / `ToolCallDelta`（避免高频 IPC 压前端）
   - 检测 `AgentEvent::Lifecycle` 的 `Stopped` 状态，收到后退出循环
   - 其余事件通过 `app.emit("kernel:subagent_event", payload)` 发送
4. Payload 格式：
   ```json
   {
     "parent_session_id": "<主 session id>",
     "parent_tool_id": "<tool call id>",
     "subagent_session_id": "<subagent session id>",
     "event": "<serde_json::Value of Event>"
   }
   ```

> 注意：后台 subscriber 不实现自动重连（out of scope）。subscriber 断开即静默结束。

---

## 前端状态层改动

### 文件：`crates/gui/frontend/src/lib/state.svelte.ts`

#### 1. 新增类型

```typescript
export interface SubagentState {
  session_id: string;
  events: SubagentEvent[];
  pending_permission: PendingPermission | null;
  pending_ask_user: PendingAskUser | null;
  is_stopped: boolean;
}

export interface SubagentEvent {
  type: "permission" | "ask_user" | "lifecycle" | "tool" | "error" | "other";
  data: unknown;
}
```

#### 2. 扩展 `ToolMessage`

```typescript
export interface ToolMessage extends BaseMessage {
  // ... 原有字段 ...
  subagent_session_id?: string;
  subagent?: SubagentState; // 运行时附加，不持久化
}
```

#### 3. 新增 `handleSubagentEvent`

```typescript
export function handleSubagentEvent(payload: {
  parent_session_id: string;
  parent_tool_id: string;
  subagent_session_id: string;
  event: unknown;
}): void {
  const session = getSession(payload.parent_session_id);
  if (!session) return;

  // 在 messages + streamingMessages 中查找对应的 ToolMessage
  const all = [...session.messages, ...(streamingMessages[session.id] ?? [])];
  const toolMsg = all.find(
    (m): m is ToolMessage =>
      m.type === "tool" && m.tool_call_id === payload.parent_tool_id
  );
  if (!toolMsg) return;

  // 初始化 subagent 状态
  if (!toolMsg.subagent) {
    toolMsg.subagent = {
      session_id: payload.subagent_session_id,
      events: [],
      pending_permission: null,
      pending_ask_user: null,
      is_stopped: false,
    };
  }
  const sub = toolMsg.subagent;

  // 解析事件
  const ev = payload.event as any;

  if (ev.agent?.permission_request) {
    const req = ev.agent.permission_request;
    sub.pending_permission = {
      req_id: req.req_id,
      tool_name: req.tool_name,
      tool_args: req.tool_args ?? "",
      tool_level: req.tool_level ?? "safe",
      reason: req.reason ?? "",
    };
    showNotification(`Subagent requests permission: ${req.tool_name}`, "warn", 5000);
  } else if (ev.agent?.ask_user_question) {
    const req = ev.agent.ask_user_question;
    sub.pending_ask_user = {
      req_id: req.req_id,
      questions: req.questions,
    };
    showNotification("Subagent has a question for you", "info", 5000);
    sendDesktopNotification("Yomi", "Subagent has a question for you", session.id);
  } else if (ev.agent?.lifecycle?.state?.stopped) {
    sub.is_stopped = true;
    sub.pending_permission = null;
    sub.pending_ask_user = null;
  } else {
    sub.events.push({ type: "other", data: ev });
  }
}
```

---

## 前端事件监听改动

### 文件：`crates/gui/frontend/src/routes/+layout.svelte`

在 `onMount` 中增加 `kernel:subagent_event` 监听：

```typescript
const unlistenSubagent = listen(
  "kernel:subagent_event",
  (e: { payload: { parent_session_id: string; parent_tool_id: string; subagent_session_id: string; event: unknown } }) => {
    handleSubagentEvent(e.payload);
  }
);

return () => {
  unlisten.then((fn: () => void) => fn());
  unlistenSubagent.then((fn: () => void) => fn());
  // ...
};
```

---

## 前端 UI 改动

### 文件：`crates/gui/frontend/src/lib/components/chat/ToolBlock.svelte`

在 `expanded` 区域末尾，subagent 输出之后，新增内联交互面板。

#### 条件渲染

```svelte
{#if tool.subagent && expanded}
  <div class="border-t border-black/5 dark:border-white/10 px-3 pb-2">
    <!-- 1. Permission 内联面板 -->
    {#if tool.subagent.pending_permission}
      <div class="my-2 rounded-md border border-amber-200 bg-amber-50/60 px-3 py-2">
        <div class="text-xs font-medium text-amber-700 mb-1.5">
          Subagent wants to run {tool.subagent.pending_permission.tool_name}
        </div>
        <div class="flex gap-1.5">
          <button onclick={() => respondSubagentPerm(tool.subagent!, false)}>
            Deny
          </button>
          <button onclick={() => respondSubagentPerm(tool.subagent!, true, false)}>
            Approve
          </button>
          <button onclick={() => respondSubagentPerm(tool.subagent!, true, true)}>
            Always
          </button>
        </div>
      </div>
    {/if}

    <!-- 2. AskUser 内联面板 -->
    {#if tool.subagent.pending_ask_user}
      <div class="my-2 rounded-md border border-blue-200 bg-blue-50/60 px-3 py-2">
        {#each tool.subagent.pending_ask_user.questions as q}
          <!-- 复用现有 AskUserBar 的选项 + textarea 样式 -->
        {/each}
        <div class="flex gap-1.5 justify-end mt-2">
          <button onclick={() => respondSubagentAskUser(tool.subagent!, [])}>
            Skip
          </button>
          <button onclick={() => respondSubagentAskUser(tool.subagent!, answers)}>
            Submit
          </button>
        </div>
      </div>
    {/if}

    <!-- 3. 运行中提示 -->
    {#if !tool.subagent.is_stopped && !tool.subagent.pending_permission && !tool.subagent.pending_ask_user}
      <div class="text-xs italic text-muted-foreground py-1">
        <Loader2 class="w-3 h-3 inline animate-spin mr-1" /> Subagent running…
      </div>
    {/if}
  </div>
{/if}
```

#### 响应函数（script 内）

```typescript
async function respondSubagentPerm(sub: SubagentState, approved: boolean, remember = false) {
  if (!sub.pending_permission) return;
  try {
    await api.respondPermission(sub.session_id, sub.pending_permission.req_id, approved, remember);
    sub.pending_permission = null;
  } catch (e) {
    showNotification("Approval failed: " + (e instanceof Error ? e.message : ""), "error", 3000);
  }
}

async function respondSubagentAskUser(sub: SubagentState, answers: [string, string][]) {
  if (!sub.pending_ask_user) return;
  try {
    await api.respondAskUser(sub.session_id, sub.pending_ask_user.req_id, answers);
    sub.pending_ask_user = null;
  } catch (e) {
    showNotification("Response failed: " + (e instanceof Error ? e.message : ""), "error", 3000);
  }
}
```

> `answers` 收集逻辑与现有 `AskUserBar.svelte` 一致：用 `selections` 和 `customInputs` 记录用户选择，提交时拼接。

---

## 关键设计决策

| 决策 | 选择 | 理由 |
|------|------|------|
| 是否把 subagent 作为独立 `SessionState`？ | **否** | 避免 sidebar 爆炸；与 TUI 对齐；用户已有跳转能力查看历史 |
| 是否复用全局 `PermissionBar`/`AskUserBar` 处理 subagent？ | **否** | 全局组件无法区分主/子 session context；内联到 ToolBlock 更直观 |
| 用户跳转到 subagent session 后，全局 PermissionBar 是否显示？ | **是** | 不做特殊处理，保持简洁。该 session 独立订阅事件，按常规逻辑渲染 |
| subagent 的 `session_id` 存在哪里？ | `ToolMessage.subagent.session_id` | 响应时必须用 subagent 自己的 `session_id` 调 API |
| 是否过滤 delta 事件？ | **是** | 避免 Tauri IPC 高频推送导致前端卡顿；TUI 也是这么做的 |
| 是否实现 auto-deny stale request？ | **否** | 当前 GUI 主 session 也未实现，本次 out of scope |
| subagent subscriber 是否自动重连？ | **否** | 保持简洁；首次实现先依赖单次订阅，断开静默结束 |

---

## 验证步骤

1. 启动一个主 session，让 agent 调用 `subagent` tool（如 `description: "请帮我写一个测试文件"`）
2. 在 subagent 内部触发一个 `permission` 请求（如 `shell` tool）
3. **验证** 主 session 的 `ToolBlock` 展开后出现 Permission 内联面板，显示工具名和 Approve/Deny 按钮
4. 点击 Approve，**验证** subagent 继续执行（可通过后端日志确认 `send_permission_response` 被调用且 `session_id` 正确）
5. 在 subagent 内部触发 `ask_user` 请求
6. **验证** 主 session 的 `ToolBlock` 展开后出现 AskUser 内联面板，显示选项和 textarea
7. 选择选项并提交，**验证** subagent 继续执行
8. 点击 `subagent` tool 的跳转按钮进入 subagent session
9. **验证** 该 session 独立渲染历史消息，不受内联面板影响
10. 若 subagent 此时还有新的 permission/ask_user，**验证** 该 session 的全局 `PermissionBar`/`AskUserBar` 正常显示（因独立事件流）
11. 等待 subagent 结束，**验证** `ToolBlock` 显示完成状态，内联面板消失

---

## 实施顺序

1. **后端**：修改 `crates/gui/src/commands/chat.rs` 的 `subscribe` 命令，增加 subagent 检测与后台 subscriber 发射
2. **前端类型**：在 `state.svelte.ts` 新增 `SubagentState`、`SubagentEvent` 类型，扩展 `ToolMessage`
3. **前端状态逻辑**：在 `state.svelte.ts` 实现 `handleSubagentEvent`
4. **前端事件监听**：在 `+layout.svelte` 增加 `kernel:subagent_event` 监听
5. **前端 UI**：在 `ToolBlock.svelte` 增加内联 Permission/AskUser 面板与响应函数
6. **测试验证**：按上述验证步骤执行端到端测试
