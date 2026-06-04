import { invoke } from "@tauri-apps/api/core";
import type { TaggedContentBlock } from "./types";

const DEFAULT_TIMEOUT = 30000; // 30s
const PING_TIMEOUT = 5000;     // 5s

async function invokeCmd<T>(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<T> {
  const promise: Promise<T> = args
    ? invoke(cmd, args as Record<string, unknown>)
    : invoke(cmd);
  return withTimeout(promise, DEFAULT_TIMEOUT, cmd);
}

async function withTimeout<T>(
  promise: Promise<T>,
  ms: number,
  label: string,
  signal?: AbortSignal,
): Promise<T> {
  let timeoutId: ReturnType<typeof setTimeout>;
  const timeout = new Promise<never>((_, reject) => {
    timeoutId = setTimeout(() => reject(new Error(`${label} timed out after ${ms}ms`)), ms);
  });

  const result = signal
    ? await Promise.race([
        promise,
        timeout,
        new Promise<never>((_, reject) => {
          signal.addEventListener("abort", () => reject(new Error(`${label} aborted`)), { once: true });
        }),
      ])
    : await Promise.race([promise, timeout]);

  clearTimeout(timeoutId!);
  return result as T;
}

// ── Project API ──────────────────────────────────────────────────────────

export interface ProjectInfo {
  id: string;
  name: string;
  dir: string;
  createdAt: string;
  updatedAt: string;
}

export async function listProjects(): Promise<ProjectInfo[]> {
  return invokeCmd("list_projects");
}

export async function createProject(dir: string, name?: string): Promise<ProjectInfo> {
  return invokeCmd("create_project", { dir, name });
}

export async function getProject(projectId: string): Promise<ProjectInfo | null> {
  return invokeCmd("get_project", { projectId });
}

export async function renameProject(projectId: string, name: string): Promise<void> {
  return invokeCmd("rename_project", { projectId, name });
}

export async function deleteProject(projectId: string): Promise<void> {
  return invokeCmd("delete_project", { projectId });
}

// ── Session API ──────────────────────────────────────────────────────────

export interface SessionInfo {
  id: string;
  projectPath: string;
  createdAt: string;
  endedAt?: string;
  title?: string;
  projectId?: string;
  autoApproveLevel?: string;
}

export interface PaginatedSessions {
  sessions: SessionInfo[];
  hasMore: boolean;
}

export async function listSessions(
  projectId?: string,
  before?: string,
  limit?: number,
): Promise<PaginatedSessions> {
  const result = await invokeCmd<{ sessions: unknown[]; hasMore: boolean }>(
    "list_sessions",
    { projectId, before, limit }
  );
  return {
    sessions: result.sessions.map((s: unknown) => {
      const session = s as Record<string, unknown>;
      return {
        id: String(session.id ?? ""),
        projectPath: String(session.workingDir ?? ""),
        createdAt: String(session.createdAt ?? ""),
        endedAt: session.updatedAt ? String(session.updatedAt) : undefined,
        title: session.title ? String(session.title) : undefined,
        projectId: session.projectId ? String(session.projectId) : undefined,
        autoApproveLevel: session.autoApproveLevel
          ? String(session.autoApproveLevel)
          : undefined,
      };
    }),
    hasMore: result.hasMore,
  };
}

export async function cancelSession(sessionId: string): Promise<void> {
  return invokeCmd("cancel_session", { sessionId });
}

export async function respondPermission(
  sessionId: string,
  reqId: string,
  approved: boolean,
  remember: boolean = false,
): Promise<void> {
  return invokeCmd("respond_permission", { sessionId, reqId, approved, remember });
}

export async function respondAskUser(
  sessionId: string,
  reqId: string,
  answers: [string, string][],
): Promise<void> {
  return invokeCmd("respond_ask_user", { sessionId, reqId, answers });
}

export async function getCwd(): Promise<string> {
  return invokeCmd("get_cwd");
}

export async function createSession(
  workingDir: string,
  level: string = "safe",
  projectId?: string,
): Promise<string> {
  return invokeCmd("create_session", { projectId, workingDir, autoApproveLevel: level });
}

export async function restoreSession(sessionId: string): Promise<void> {
  return invokeCmd("restore_session", { sessionId });
}

export async function forkSession(
  parentId: string,
  level: string = "safe",
): Promise<string> {
  return invokeCmd("fork_session", { parentId, autoApproveLevel: level });
}

export async function deleteSession(sessionId: string): Promise<void> {
  return invokeCmd("delete_session", { sessionId });
}

export async function shutdownSession(sessionId: string): Promise<void> {
  return invokeCmd("shutdown_session", { sessionId });
}

export async function sendMessage(sessionId: string, content: string): Promise<void> {
  return invokeCmd("send_message", { sessionId, content });
}

export async function sendMessageBlocks(sessionId: string, blocks: TaggedContentBlock[]): Promise<void> {
  return invokeCmd("send_message_blocks", { sessionId, blocks });
}

export async function subscribe(sessionId: string): Promise<void> {
  return invokeCmd("subscribe", { sessionId });
}

export async function unsubscribe(sessionId: string): Promise<void> {
  return invokeCmd("unsubscribe", { sessionId });
}

export async function getMessages(sessionId: string): Promise<unknown[]> {
  return invokeCmd("get_messages", { sessionId });
}

export async function getCheckpoints(sessionId: string): Promise<unknown[]> {
  return invokeCmd("get_checkpoints", { sessionId });
}

export async function rewind(sessionId: string, messageId: string): Promise<void> {
  return invokeCmd("rewind", { sessionId, messageId });
}

export async function listSkills(): Promise<unknown[]> {
  return invokeCmd("list_skills");
}

export async function reloadConfig(): Promise<void> {
  return invokeCmd("reload_config");
}

export async function compactSession(sessionId: string): Promise<void> {
  return invokeCmd("compact_session", { sessionId });
}

export async function setPermissionLevel(sessionId: string, level: string): Promise<void> {
  return invokeCmd("set_permission_level", { sessionId, level });
}

export async function startGoal(sessionId: string, description: string): Promise<void> {
  return invokeCmd("start_goal", { sessionId, description });
}

export async function stopGoal(sessionId: string): Promise<void> {
  return invokeCmd("stop_goal", { sessionId });
}

export async function sendSteer(sessionId: string, blocks: TaggedContentBlock[]): Promise<void> {
  return invokeCmd("send_steer", { sessionId, blocks });
}

export async function getConfigToml(): Promise<{ content: string; path: string }> {
  return invokeCmd("get_config_toml");
}

export async function saveConfigToml(content: string): Promise<void> {
  return invokeCmd("save_config_toml", { content });
}

export async function getConfig(): Promise<{
  model: string;
  contextWindow: number;
  provider: string;
  autoApprove: string;
  fullConfig: string;
}> {
  return invokeCmd("get_config");
}

export async function getUsageSummary(): Promise<{
  promptTokens: number;
  completionTokens: number;
  cachedTokens: number;
  totalTokens: number;
  requestCount: number;
}> {
  return invokeCmd("get_usage_summary");
}

export async function getDailyUsage(days: number): Promise<
  {
    date: string;
    promptTokens: number;
    completionTokens: number;
    cachedTokens: number;
    totalTokens: number;
    requestCount: number;
    models: string[];
  }[]
> {
  return invokeCmd("get_daily_usage", { days });
}

export async function getSessionUsage(sessionId: string): Promise<{
  promptTokens: number;
  completionTokens: number;
  cachedTokens: number;
  totalTokens: number;
  requestCount: number;
}> {
  return invokeCmd("get_session_usage", { sessionId });
}

export async function getTodos(sessionId: string): Promise<{
  todos: { id: string; content: string; status: string }[];
}> {
  return invokeCmd("get_todos", { sessionId });
}

export async function renameSession(sessionId: string, title: string): Promise<void> {
  return invokeCmd("rename_session", { sessionId, title });
}

export async function ping(): Promise<boolean> {
  return withTimeout(invoke("ping"), PING_TIMEOUT, "ping");
}
export async function openInExplorer(path: string): Promise<void> {
  return invokeCmd("open_in_explorer", { path });
}

export async function openInVscode(path: string): Promise<void> {
  return invokeCmd("open_in_vscode", { path });
}

export async function openInZed(path: string): Promise<void> {
  return invokeCmd("open_in_zed", { path });
}

export async function openInEditor(path: string): Promise<void> {
  return invokeCmd("open_in_editor", { path });
}

// ─── Cron / Automation ──────────────────────────────────

export async function listCronJobs(
  status?: string,
  limit = 100,
): Promise<unknown[]> {
  return invokeCmd("list_cron_jobs", { status, limit });
}

export async function createCronJob(input: {
  name: string;
  schedule: string;
  action: Record<string, unknown>;
  maxRuns?: number;
  expiresAt?: string;
}): Promise<string> {
  return invokeCmd("create_cron_job", input);
}

export async function updateCronJob(
  jobId: string,
  input: {
    name?: string;
    schedule?: string;
    action?: Record<string, unknown>;
    status?: string;
    maxRuns?: number;
    expiresAt?: string;
  },
): Promise<void> {
  return invokeCmd("update_cron_job", { jobId, ...input });
}

export async function deleteCronJob(jobId: string): Promise<void> {
  return invokeCmd("delete_cron_job", { jobId });
}

export async function triggerCronJob(jobId: string): Promise<void> {
  return invokeCmd("trigger_cron_job", { jobId });
}
