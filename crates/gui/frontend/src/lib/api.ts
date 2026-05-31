import { invoke } from "@tauri-apps/api/core";

const DEFAULT_TIMEOUT = 30000; // 30s
const PING_TIMEOUT = 5000;     // 5s

async function withTimeout<T>(promise: Promise<T>, ms: number, label: string): Promise<T> {
  const timeout = new Promise<never>((_, reject) =>
    setTimeout(() => reject(new Error(`${label} timed out after ${ms}ms`)), ms)
  );
  return Promise.race([promise, timeout]);
}

export interface SessionInfo {
  id: string;
  projectPath: string;
  createdAt: string;
  endedAt?: string;
}

export async function listSessions(): Promise<SessionInfo[]> {
  return withTimeout(invoke("list_sessions"), DEFAULT_TIMEOUT, "list_sessions");
}

export async function getCwd(): Promise<string> {
  return withTimeout(invoke("get_cwd"), DEFAULT_TIMEOUT, "get_cwd");
}

export async function createSession(
  projectPath: string,
  level: string = "safe"
): Promise<string> {
  return withTimeout(
    invoke("create_session", { projectPath, autoApproveLevel: level }),
    DEFAULT_TIMEOUT,
    "create_session"
  );
}

export async function restoreSession(
  sessionId: string,
  level: string = "safe"
): Promise<void> {
  return withTimeout(
    invoke("restore_session", { sessionId, autoApproveLevel: level }),
    DEFAULT_TIMEOUT,
    "restore_session"
  );
}

export async function forkSession(
  parentId: string,
  level: string = "safe"
): Promise<string> {
  return withTimeout(
    invoke("fork_session", { parentId, autoApproveLevel: level }),
    DEFAULT_TIMEOUT,
    "fork_session"
  );
}

export async function deleteSession(sessionId: string): Promise<void> {
  return withTimeout(
    invoke("delete_session", { sessionId }),
    DEFAULT_TIMEOUT,
    "delete_session"
  );
}

export async function shutdownSession(sessionId: string): Promise<void> {
  return withTimeout(
    invoke("shutdown_session", { sessionId }),
    DEFAULT_TIMEOUT,
    "shutdown_session"
  );
}

export async function sendMessage(
  sessionId: string,
  content: string
): Promise<void> {
  return withTimeout(
    invoke("send_message", { sessionId, content }),
    DEFAULT_TIMEOUT,
    "send_message"
  );
}

export async function subscribe(sessionId: string, level: string = "safe"): Promise<void> {
  return withTimeout(
    invoke("subscribe", { sessionId, autoApproveLevel: level }),
    DEFAULT_TIMEOUT,
    "subscribe"
  );
}

export async function unsubscribe(sessionId: string): Promise<void> {
  return withTimeout(
    invoke("unsubscribe", { sessionId }),
    DEFAULT_TIMEOUT,
    "unsubscribe"
  );
}

export async function getMessages(sessionId: string): Promise<unknown[]> {
  return withTimeout(
    invoke("get_messages", { sessionId }),
    DEFAULT_TIMEOUT,
    "get_messages"
  );
}

export async function getCheckpoints(sessionId: string): Promise<unknown[]> {
  return withTimeout(
    invoke("get_checkpoints", { sessionId }),
    DEFAULT_TIMEOUT,
    "get_checkpoints"
  );
}

export async function rewind(sessionId: string, messageId: string): Promise<void> {
  return withTimeout(
    invoke("rewind", { sessionId, messageId }),
    DEFAULT_TIMEOUT,
    "rewind"
  );
}

export async function listSkills(): Promise<unknown[]> {
  return withTimeout(invoke("list_skills"), DEFAULT_TIMEOUT, "list_skills");
}

export async function reloadConfig(): Promise<void> {
  return withTimeout(invoke("reload_config"), DEFAULT_TIMEOUT, "reload_config");
}

export async function ping(): Promise<boolean> {
  return withTimeout(invoke("ping"), PING_TIMEOUT, "ping");
}
