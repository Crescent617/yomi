import { invoke } from "@tauri-apps/api/core";

export interface SessionInfo {
  id: string;
  projectPath: string;
  createdAt: string;
  endedAt?: string;
}

export async function listSessions(): Promise<SessionInfo[]> {
  return invoke("list_sessions");
}

export async function createSession(
  projectPath: string,
  level: string = "safe"
): Promise<string> {
  return invoke("create_session", { projectPath, autoApproveLevel: level });
}

export async function restoreSession(
  sessionId: string,
  level: string = "safe"
): Promise<void> {
  return invoke("restore_session", { sessionId, autoApproveLevel: level });
}

export async function forkSession(
  parentId: string,
  level: string = "safe"
): Promise<string> {
  return invoke("fork_session", { parentId, autoApproveLevel: level });
}

export async function deleteSession(sessionId: string): Promise<void> {
  return invoke("delete_session", { sessionId });
}

export async function shutdownSession(sessionId: string): Promise<void> {
  return invoke("shutdown_session", { sessionId });
}

export async function sendMessage(
  sessionId: string,
  content: string
): Promise<void> {
  return invoke("send_message", { sessionId, content });
}

export async function subscribe(sessionId: string): Promise<void> {
  return invoke("subscribe", { sessionId });
}

export async function unsubscribe(sessionId: string): Promise<void> {
  return invoke("unsubscribe", { sessionId });
}

export async function getMessages(sessionId: string): Promise<unknown[]> {
  return invoke("get_messages", { sessionId });
}

export async function getCheckpoints(sessionId: string): Promise<unknown[]> {
  return invoke("get_checkpoints", { sessionId });
}

export async function rewind(sessionId: string, messageId: string): Promise<void> {
  return invoke("rewind", { sessionId, messageId });
}

export async function listSkills(): Promise<unknown[]> {
  return invoke("list_skills");
}

export async function reloadConfig(): Promise<void> {
  return invoke("reload_config");
}

export async function ping(): Promise<boolean> {
  return invoke("ping");
}