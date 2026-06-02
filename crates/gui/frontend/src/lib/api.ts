import { invoke } from "@tauri-apps/api/core";

const DEFAULT_TIMEOUT = 30000; // 30s
const PING_TIMEOUT = 5000;     // 5s

async function withTimeout<T>(promise: Promise<T>, ms: number, label: string): Promise<T> {
  let timeoutId: ReturnType<typeof setTimeout>;
  const timeout = new Promise<never>((_, reject) => {
    timeoutId = setTimeout(() => reject(new Error(`${label} timed out after ${ms}ms`)), ms);
  });
  const result = await Promise.race([promise, timeout]);
  clearTimeout(timeoutId!);
  return result;
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
  return withTimeout(invoke("list_projects"), DEFAULT_TIMEOUT, "list_projects");
}

export async function createProject(dir: string, name?: string): Promise<ProjectInfo> {
  return withTimeout(
    invoke("create_project", { dir, name }),
    DEFAULT_TIMEOUT,
    "create_project"
  );
}

export async function getProject(projectId: string): Promise<ProjectInfo | null> {
  return withTimeout(
    invoke("get_project", { projectId }),
    DEFAULT_TIMEOUT,
    "get_project"
  );
}

export async function renameProject(projectId: string, name: string): Promise<void> {
  return withTimeout(
    invoke("rename_project", { projectId, name }),
    DEFAULT_TIMEOUT,
    "rename_project"
  );
}

export async function deleteProject(projectId: string): Promise<void> {
  return withTimeout(
    invoke("delete_project", { projectId }),
    DEFAULT_TIMEOUT,
    "delete_project"
  );
}

// ── Session API ──────────────────────────────────────────────────────────

export interface SessionInfo {
  id: string;
  projectPath: string;
  createdAt: string;
  endedAt?: string;
  title?: string;
  projectId?: string;
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
  const result = await withTimeout(
    invoke<{ sessions: unknown[]; has_more: boolean }>("list_sessions", { projectId, before, limit }),
    DEFAULT_TIMEOUT,
    "list_sessions",
  );
  return {
    sessions: result.sessions.map((s: any) => ({
      id: s.id,
      projectPath: s.workingDir ?? "",
      createdAt: s.created_at,
      endedAt: s.updated_at,
      title: s.title,
      projectId: s.projectId,
    })),
    hasMore: result.has_more,
  };
}

export async function cancelSession(sessionId: string): Promise<void> {
  return withTimeout(
    invoke("cancel_session", { sessionId }),
    DEFAULT_TIMEOUT,
    "cancel_session",
  );
}

export async function respondPermission(
  sessionId: string,
  reqId: string,
  approved: boolean,
  remember: boolean = false,
): Promise<void> {
  return withTimeout(
    invoke("respond_permission", { sessionId, reqId, approved, remember }),
    DEFAULT_TIMEOUT,
    "respond_permission",
  );
}

export async function respondAskUser(
  sessionId: string,
  reqId: string,
  answers: [string, string][],
): Promise<void> {
  return withTimeout(
    invoke("respond_ask_user", { sessionId, reqId, answers }),
    DEFAULT_TIMEOUT,
    "respond_ask_user",
  );
}

export async function getCwd(): Promise<string> {
  return withTimeout(invoke("get_cwd"), DEFAULT_TIMEOUT, "get_cwd");
}

export async function createSession(
  workingDir: string,
  level: string = "safe",
  projectId?: string,
): Promise<string> {
  return withTimeout(
    invoke("create_session", { projectId, workingDir, autoApproveLevel: level }),
    DEFAULT_TIMEOUT,
    "create_session",
  );
}

export async function restoreSession(
  sessionId: string,
  level: string = "safe",
): Promise<void> {
  return withTimeout(
    invoke("restore_session", { sessionId, autoApproveLevel: level }),
    DEFAULT_TIMEOUT,
    "restore_session",
  );
}

export async function forkSession(
  parentId: string,
  level: string = "safe",
): Promise<string> {
  return withTimeout(
    invoke("fork_session", { parentId, autoApproveLevel: level }),
    DEFAULT_TIMEOUT,
    "fork_session",
  );
}

export async function deleteSession(sessionId: string): Promise<void> {
  return withTimeout(
    invoke("delete_session", { sessionId }),
    DEFAULT_TIMEOUT,
    "delete_session",
  );
}

export async function shutdownSession(sessionId: string): Promise<void> {
  return withTimeout(
    invoke("shutdown_session", { sessionId }),
    DEFAULT_TIMEOUT,
    "shutdown_session",
  );
}

export async function sendMessage(
  sessionId: string,
  content: string,
): Promise<void> {
  return withTimeout(
    invoke("send_message", { sessionId, content }),
    DEFAULT_TIMEOUT,
    "send_message",
  );
}

export async function subscribe(sessionId: string, level: string = "safe"): Promise<void> {
  return withTimeout(
    invoke("subscribe", { sessionId, autoApproveLevel: level }),
    DEFAULT_TIMEOUT,
    "subscribe",
  );
}

export async function unsubscribe(sessionId: string): Promise<void> {
  return withTimeout(
    invoke("unsubscribe", { sessionId }),
    DEFAULT_TIMEOUT,
    "unsubscribe",
  );
}

export async function getMessages(sessionId: string): Promise<unknown[]> {
  return withTimeout(
    invoke("get_messages", { sessionId }),
    DEFAULT_TIMEOUT,
    "get_messages",
  );
}

export async function getCheckpoints(sessionId: string): Promise<unknown[]> {
  return withTimeout(
    invoke("get_checkpoints", { sessionId }),
    DEFAULT_TIMEOUT,
    "get_checkpoints",
  );
}

export async function rewind(sessionId: string, messageId: string): Promise<void> {
  return withTimeout(
    invoke("rewind", { sessionId, messageId }),
    DEFAULT_TIMEOUT,
    "rewind",
  );
}

export async function listSkills(): Promise<unknown[]> {
  return withTimeout(invoke("list_skills"), DEFAULT_TIMEOUT, "list_skills");
}

export async function reloadConfig(): Promise<void> {
  return withTimeout(invoke("reload_config"), DEFAULT_TIMEOUT, "reload_config");
}

export async function compactSession(sessionId: string): Promise<void> {
  return withTimeout(
    invoke("compact_session", { sessionId }),
    DEFAULT_TIMEOUT,
    "compact_session",
  );
}

export async function setPermissionLevel(sessionId: string, level: string): Promise<void> {
  return withTimeout(
    invoke("set_permission_level", { sessionId, level }),
    DEFAULT_TIMEOUT,
    "set_permission_level",
  );
}

export async function startGoal(sessionId: string, description: string): Promise<void> {
  return withTimeout(
    invoke("start_goal", { sessionId, description }),
    DEFAULT_TIMEOUT,
    "start_goal",
  );
}

export async function stopGoal(sessionId: string): Promise<void> {
  return withTimeout(
    invoke("stop_goal", { sessionId }),
    DEFAULT_TIMEOUT,
    "stop_goal",
  );
}

export async function getConfig(): Promise<{ model: string; context_window: number }> {
  return withTimeout(invoke("get_config"), DEFAULT_TIMEOUT, "get_config");
}

export async function ping(): Promise<boolean> {
  return withTimeout(invoke("ping"), PING_TIMEOUT, "ping");
}
