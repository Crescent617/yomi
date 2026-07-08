import { invoke } from "@tauri-apps/api/core";
import type { TaggedContentBlock } from "./types";

const DEFAULT_TIMEOUT = 30000; // 30s
const PING_TIMEOUT = 5000; // 5s

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
    timeoutId = setTimeout(
      () => reject(new Error(`${label} timed out after ${ms}ms`)),
      ms,
    );
  });

  const result = signal
    ? await Promise.race([
        promise,
        timeout,
        new Promise<never>((_, reject) => {
          signal.addEventListener(
            "abort",
            () => reject(new Error(`${label} aborted`)),
            { once: true },
          );
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
  created_at: string;
  updated_at: string;
}

export async function listProjects(): Promise<ProjectInfo[]> {
  return invokeCmd("list_projects");
}

export async function createProject(
  dir: string,
  name?: string,
): Promise<ProjectInfo> {
  return invokeCmd("create_project", { dir, name });
}

export async function getProject(
  project_id: string,
): Promise<ProjectInfo | null> {
  return invokeCmd("get_project", { project_id: project_id });
}

export async function renameProject(
  project_id: string,
  name: string,
): Promise<void> {
  return invokeCmd("rename_project", { project_id: project_id, name });
}

export async function deleteProject(project_id: string): Promise<void> {
  return invokeCmd("delete_project", { project_id: project_id });
}

// ── Session API ──────────────────────────────────────────────────────────

export interface SessionInfo {
  id: string;
  project_path: string;
  created_at: string;
  updated_at?: string;
  title?: string;
  project_id?: string;
  auto_approve_level?: string;
}

export interface PinnedSessionDetail {
  session_id: string;
  title?: string;
  project_id?: string;
  updated_at: string;
  pinned_at: string;
}

export interface PaginatedSessions {
  sessions: SessionInfo[];
  next_cursor: string | null;
}

export async function listSessions(
  project_id?: string,
  before?: string,
  limit?: number,
): Promise<PaginatedSessions> {
  const result = await invokeCmd<{
    sessions: unknown[];
    next_cursor: string | null;
  }>("list_sessions", { project_id: project_id, before, limit });
  return {
    sessions: result.sessions.map((s: unknown) => {
      const session = s as Record<string, unknown>;
      return {
        id: String(session.id ?? ""),
        project_path: String(session.working_dir ?? ""),
        created_at: String(session.created_at ?? ""),
        updated_at: session.updated_at ? String(session.updated_at) : undefined,
        title: session.title ? String(session.title) : undefined,
        project_id: session.project_id ? String(session.project_id) : undefined,
        auto_approve_level: session.auto_approve_level
          ? String(session.auto_approve_level)
          : undefined,
      };
    }),
    next_cursor: result.next_cursor,
  };
}

export async function cancelSession(session_id: string): Promise<void> {
  return invokeCmd("cancel_session", { session_id: session_id });
}

export async function respondPermission(
  session_id: string,
  req_id: string,
  approved: boolean,
  remember: boolean = false,
): Promise<void> {
  return invokeCmd("respond_permission", {
    session_id: session_id,
    req_id: req_id,
    approved,
    remember,
  });
}

export async function respondAskUser(
  session_id: string,
  req_id: string,
  answers: [string, string][],
): Promise<void> {
  return invokeCmd("respond_ask_user", {
    session_id: session_id,
    req_id: req_id,
    answers,
  });
}

export async function getCwd(): Promise<string> {
  return invokeCmd("get_cwd");
}

export async function createSession(
  working_dir: string,
  level: string = "safe",
  project_id?: string,
): Promise<string> {
  return invokeCmd("create_session", {
    project_id: project_id,
    working_dir: working_dir,
    auto_approve_level: level,
  });
}

export async function restoreSession(session_id: string): Promise<void> {
  return invokeCmd("restore_session", { session_id: session_id });
}

export async function forkSession(
  parent_id: string,
  level: string = "safe",
): Promise<string> {
  return invokeCmd("fork_session", {
    parent_id: parent_id,
    auto_approve_level: level,
  });
}

export async function deleteSession(session_id: string): Promise<void> {
  return invokeCmd("delete_session", { session_id: session_id });
}

export async function shutdownSession(session_id: string): Promise<void> {
  return invokeCmd("shutdown_session", { session_id: session_id });
}

export async function sendMessage(
  session_id: string,
  content: string,
): Promise<void> {
  return invokeCmd("send_message", { session_id: session_id, content });
}

export async function sendMessageBlocks(
  session_id: string,
  blocks: TaggedContentBlock[],
): Promise<void> {
  return invokeCmd("send_message_blocks", { session_id: session_id, blocks });
}

export async function subscribe(
  session_id: string,
  after_event_id?: string | null,
): Promise<void> {
  return invokeCmd("subscribe", {
    session_id: session_id,
    after_event_id: after_event_id ?? null,
  });
}

export async function unsubscribe(session_id: string): Promise<void> {
  return invokeCmd("unsubscribe", { session_id: session_id });
}

// SessionMessage types from kernel::list_messages (tagged union via kind)
export interface SessionMessageUser {
  kind: "user";
  id: string;
  content: TaggedContentBlock[];
  created_at: string;
}

export interface SessionMessageAssistant {
  kind: "assistant";
  id: string;
  content: TaggedContentBlock[];
  tool_calls: { id: string; name: string; arguments: string }[] | null;
  token_usage: {
    prompt_tokens: number;
    completion_tokens: number;
    total_tokens: number;
  } | null;
  response_id: string | null;
  finish_reason: string | null;
  created_at: string;
}

export interface SessionMessageTool {
  kind: "tool";
  id: string;
  tool_call_id: string;
  name: string;
  args: string;
  result: TaggedContentBlock[];
  meta: Record<string, string>;
  created_at: string;
}

export type SessionMessage =
  | SessionMessageUser
  | SessionMessageAssistant
  | SessionMessageTool;

export async function getMessages(
  session_id: string,
): Promise<SessionMessage[]> {
  return invokeCmd("get_messages", { session_id: session_id });
}

export async function getSession(session_id: string): Promise<{
  id: string;
  phase: string;
  title: string | null;
  parent_id: string | null;
  project_id: string | null;
  working_dir: string | null;
  message_count: number;
  created_at: string;
  updated_at: string;
  auto_approve_level: string | null;
}> {
  return invokeCmd("get_session", { session_id: session_id });
}

export async function getCheckpoints(session_id: string): Promise<unknown[]> {
  return invokeCmd("get_checkpoints", { session_id: session_id });
}

export async function rewind(
  session_id: string,
  message_id: string,
): Promise<void> {
  return invokeCmd("rewind", {
    session_id: session_id,
    message_id: message_id,
  });
}

export interface SkillInfo {
  name: string;
  description: string;
}

export async function listSessionSkills(
  session_id: string,
): Promise<SkillInfo[]> {
  return invokeCmd<SkillInfo[]>("list_session_skills", {
    session_id: session_id,
  });
}

export async function compactSession(session_id: string): Promise<void> {
  return invokeCmd("compact_session", { session_id: session_id });
}

export async function setPermissionLevel(
  session_id: string,
  level: string,
): Promise<void> {
  return invokeCmd("set_permission_level", { session_id: session_id, level });
}

export async function startGoal(
  session_id: string,
  description: string,
): Promise<void> {
  return invokeCmd("start_goal", { session_id: session_id, description });
}

export async function getGoal(
  session_id: string,
): Promise<{ description: string; status: string } | null> {
  return invokeCmd("get_goal", { session_id: session_id });
}

export async function stopGoal(session_id: string): Promise<void> {
  return invokeCmd("stop_goal", { session_id: session_id });
}

export async function pauseGoal(session_id: string): Promise<void> {
  return invokeCmd("pause_goal", { session_id: session_id });
}

export async function resumeGoal(session_id: string): Promise<void> {
  return invokeCmd("resume_goal", { session_id: session_id });
}

export async function editGoal(
  session_id: string,
  description: string,
): Promise<void> {
  return invokeCmd("edit_goal", { session_id: session_id, description });
}

export async function sendSteer(
  session_id: string,
  blocks: TaggedContentBlock[],
): Promise<void> {
  return invokeCmd("send_steer", { session_id: session_id, blocks });
}

export async function continueSession(session_id: string): Promise<void> {
  return invokeCmd("continue_session", { session_id: session_id });
}

export async function getConfigToml(): Promise<{
  content: string;
  path: string;
}> {
  return invokeCmd("get_config_toml");
}

export async function saveConfigToml(content: string): Promise<void> {
  return invokeCmd("save_config_toml", { content });
}

export async function getConfig(): Promise<{
  model: string;
  context_window: number;
  provider: string;
  auto_approve: string;
  full_config: string;
}> {
  return invokeCmd("get_config");
}

export async function getUsageSummary(): Promise<{
  prompt_tokens: number;
  completion_tokens: number;
  cached_tokens: number;
  request_count: number;
}> {
  return invokeCmd("get_usage_summary");
}

export async function getDailyUsage(days: number): Promise<
  {
    date: string;
    prompt_tokens: number;
    completion_tokens: number;
    cached_tokens: number;
    request_count: number;
    models: string[];
  }[]
> {
  return invokeCmd("get_daily_usage", { days });
}

export async function getTodos(session_id: string): Promise<{
  todos: { id: string; content: string; status: string }[];
}> {
  return invokeCmd("get_todos", { session_id: session_id });
}

export async function renameSession(
  session_id: string,
  title: string,
): Promise<void> {
  return invokeCmd("rename_session", { session_id: session_id, title });
}

export async function pinSession(session_id: string): Promise<void> {
  return invokeCmd("pin_session", {
    session_id: session_id,
    icon_emoji: null,
  });
}

export async function unpinSession(session_id: string): Promise<void> {
  return invokeCmd("unpin_session", { session_id: session_id });
}

export async function listPinnedSessions(): Promise<PinnedSessionDetail[]> {
  return invokeCmd<PinnedSessionDetail[]>("list_pinned_sessions");
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

export interface GitInfo {
  branch?: string | null;
  added_lines: number;
  deleted_lines: number;
  untracked: number;
  repo_root?: string;
}

export interface GitDiffFileSummary {
  path: string;
  status: string;
}

export async function getGitDiffSummary(
  path: string,
  staged: boolean,
): Promise<GitDiffFileSummary[] | null> {
  return invokeCmd<GitDiffFileSummary[] | null>("get_git_diff_summary", {
    path,
    staged,
  });
}

export async function getGitFileDiffRaw(
  path: string,
  file_path: string,
  staged: boolean,
): Promise<string | null> {
  return invokeCmd<string | null>("get_git_file_diff_raw", {
    path,
    file_path,
    staged,
  });
}

const inflightGit = new Map<string, Promise<GitInfo | null>>();

export async function getGitInfo(path: string): Promise<GitInfo | null> {
  const existing = inflightGit.get(path);
  if (existing) return existing;

  const promise = invokeCmd<GitInfo | null>("get_git_info", { path }).finally(
    () => inflightGit.delete(path),
  );

  inflightGit.set(path, promise);
  return promise;
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
  max_runs?: number;
  expires_at?: string;
}): Promise<string> {
  return invokeCmd("create_cron_job", input);
}

export async function updateCronJob(
  job_id: string,
  input: {
    name?: string;
    schedule?: string;
    action?: Record<string, unknown>;
    status?: string;
    max_runs?: number;
    expires_at?: string;
  },
): Promise<void> {
  return invokeCmd("update_cron_job", { job_id: job_id, ...input });
}

export async function deleteCronJob(job_id: string): Promise<void> {
  return invokeCmd("delete_cron_job", { job_id: job_id });
}

export async function triggerCronJob(job_id: string): Promise<void> {
  return invokeCmd("trigger_cron_job", { job_id: job_id });
}
