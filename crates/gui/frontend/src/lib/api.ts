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

export function errorMessage(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === "string") return e;
  const recordE = e as Record<string, unknown>;
  if (typeof recordE?.message === "string") return recordE.message;
  try {
    return JSON.stringify(e);
  } catch {
    return String(e);
  }
}

export interface DebugFileChunk {
  content: string;
  path: string;
  file_size: number;
  start_offset: number;
  end_offset: number;
  has_earlier: boolean;
}

export async function listGuiLogs(): Promise<string[]> {
  return invokeCmd<string[]>("list_gui_logs");
}

export async function readSessionJsonl(
  session_id: string,
  before_offset?: number,
  after_offset?: number,
): Promise<DebugFileChunk> {
  return invokeCmd<DebugFileChunk>("read_session_jsonl", {
    session_id,
    before_offset,
    after_offset,
  });
}

export async function readGuiLog(
  file_name: string,
  before_offset?: number,
  after_offset?: number,
): Promise<DebugFileChunk> {
  return invokeCmd<DebugFileChunk>("read_gui_log", {
    file_name,
    before_offset,
    after_offset,
  });
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

export interface DeleteProjectResult {
  sessions_deleted: number;
  bytes_reclaimed: number;
}

/** Deletes the project AND all its sessions (incl. subagents) with their
 *  resources. Caller must confirm with the user first. */
export async function deleteProject(
  project_id: string,
): Promise<DeleteProjectResult> {
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
  model_key?: string;
}

export interface BackgroundShellTask {
  task_id: string;
  session_id: string;
  pid: number;
  command: string;
  output_path: string;
  started_at: string;
}

export interface RunningSessionInfo {
  id: string;
  parent_id: string | null;
  title: string | null;
  project_id: string | null;
  phase: string;
  background_task_count: number;
  background_shells: BackgroundShellTask[];
}

export async function listRunningSessions(): Promise<RunningSessionInfo[]> {
  return invokeCmd<RunningSessionInfo[]>("list_running_sessions");
}

export interface SubagentInfo {
  id: string;
  parent_session_id: string;
  alias: string | null;
  phase: string;
  created_at: string;
  model_key: string | null;
}

export async function listSubagents(
  parent_session_id: string,
): Promise<SubagentInfo[]> {
  return invokeCmd<SubagentInfo[]>("list_subagents", {
    parent_session_id,
  });
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
        model_key: session.model_key ? String(session.model_key) : undefined,
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

// ── Desktop pet API ───────────────────────────────────────────────────────

export interface PetPermissionRequest {
  kind: "permission";
  req_id: string;
  session_id: string;
  title: string;
}

export interface PetAskUserRequest {
  kind: "ask_user";
  req_id: string;
  session_id: string;
  title: string;
}

export type PetRequest = PetPermissionRequest | PetAskUserRequest;
export type PetNoticeKind =
  | "completed"
  | "cancelled"
  | "failed"
  | "max_iterations";

export interface PetNotice {
  event_id: string;
  session_id: string;
  title: string;
  kind: PetNoticeKind;
  message: string | null;
}

export type PetMood =
  | "idle"
  | "working"
  | "happy"
  | "curious"
  | "alert"
  | "worried"
  | "sleepy";

export interface PetSnapshot {
  revision: number;
  connection_status: "connected" | "disconnected";
  running_count: number;
  mood: PetMood;
  request: PetRequest | null;
  notice: PetNotice | null;
}

export interface PetPack {
  id: string;
  display_name: string;
  description: string;
  kind: string | null;
  sprite_version_number: 1 | 2;
}

export async function listPetPacks(): Promise<PetPack[]> {
  return invokeCmd("list_pet_packs");
}

export async function selectPetPack(pet_id: string | null): Promise<void> {
  return invokeCmd("select_pet_pack", { pet_id });
}

export async function getSelectedPetPack(): Promise<PetPack | null> {
  return invokeCmd("get_selected_pet_pack");
}

export async function readSelectedPetSpritesheet(
  pet_id: string,
  sprite_version_number: 1 | 2,
): Promise<Uint8Array<ArrayBuffer>> {
  const bytes = await invoke<ArrayBuffer | number[]>(
    "read_selected_pet_spritesheet",
    { pet_id, sprite_version_number },
  );
  return Array.isArray(bytes) ? new Uint8Array(bytes) : new Uint8Array(bytes);
}

export async function getPetScale(): Promise<number> {
  return invokeCmd("get_pet_scale");
}

export async function setPetScale(scale: number): Promise<void> {
  return invokeCmd("set_pet_scale", { scale });
}

export async function getPetState(): Promise<PetSnapshot> {
  return invokeCmd("get_pet_state");
}

export async function setPetEnabled(enabled: boolean): Promise<void> {
  return invokeCmd("set_pet_enabled", { enabled });
}

export async function getCwd(): Promise<string> {
  return invokeCmd("get_cwd");
}

export async function createSession(
  working_dir: string,
  level: string = "safe",
  project_id?: string,
  model_key?: string,
): Promise<string> {
  return invokeCmd("create_session", {
    project_id: project_id,
    working_dir: working_dir,
    auto_approve_level: level,
    model_key: model_key ?? null,
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

export async function clearSession(session_id: string): Promise<void> {
  return invokeCmd("clear_session", { session_id: session_id });
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

export interface SessionMessageSteer {
  kind: "steer";
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
  model_id: string | null;
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
  | SessionMessageSteer
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
  model_key: string | null;
}> {
  return invokeCmd("get_session", { session_id: session_id });
}

export interface Checkpoint {
  id: string;
  session_id: string;
  message_id: string;
  sequence: number;
  created_at: number;
  files_changed: number;
  summary: string;
}

export async function getCheckpoints(
  session_id: string,
): Promise<Checkpoint[]> {
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

export async function getDaemonStatus(): Promise<{ managed: boolean }> {
  return invokeCmd("get_daemon_status");
}

export async function restartDaemon(): Promise<void> {
  return invokeCmd("restart_daemon");
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

export interface ModelUsage {
  model: string;
  provider: string;
  prompt_tokens: number;
  completion_tokens: number;
  cached_tokens: number;
  request_count: number;
}

export async function getModelUsage(days: number): Promise<ModelUsage[]> {
  return invokeCmd("get_model_usage", { days });
}

export async function getTodayModelUsage(): Promise<ModelUsage[]> {
  return invokeCmd("get_today_model_usage");
}

export interface UsageRecord {
  id: string;
  session_id: string;
  prompt_tokens: number;
  completion_tokens: number;
  cached_tokens: number;
  model: string;
  provider: string;
  usage_type: string;
  created_at: string;
}

export async function getUsageRecords(
  before_id?: string,
  limit?: number,
): Promise<UsageRecord[]> {
  return invokeCmd("get_usage_records", { before_id, limit });
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
export async function openDefault(target: string): Promise<void> {
  return invokeCmd("open_default", { target });
}

export async function openInVscode(path: string): Promise<void> {
  return invokeCmd("open_in_vscode", { path });
}

export async function openInZed(path: string): Promise<void> {
  return invokeCmd("open_in_zed", { path });
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

// ─── Model API ──────────────────────────────────────────────────────

export interface ModelInfo {
  name: string;
  model_id: string;
  provider: string;
  context_window: number;
}

export async function getModels(): Promise<{ models: ModelInfo[] }> {
  return invokeCmd("get_models");
}

export async function getSessionModel(session_id: string): Promise<string> {
  return invokeCmd("get_session_model", { session_id });
}

export async function setSessionModel(
  session_id: string,
  key: string,
): Promise<void> {
  return invokeCmd("set_session_model", { session_id, key });
}

// ─── Cron / Automation ──────────────────────────────────

export interface CronAction {
  type: string;
  session_id?: string;
  content?: string;
  command?: string;
  working_dir?: string;
}

export interface CronJob {
  id: string;
  name: string;
  schedule: string;
  action: CronAction;
  status: "active" | "paused" | "completed" | "failed";
  created_at: string;
  updated_at: string;
  next_run_at: string | null;
  last_run_at: string | null;
  run_count: number;
  max_runs: number | null;
  expires_at: string | null;
  last_error: string | null;
}

export async function listCronJobs(
  status?: string,
  limit = 100,
): Promise<CronJob[]> {
  return invokeCmd("list_cron_jobs", { status, limit });
}

export async function createCronJob(input: {
  name: string;
  schedule: string;
  action: string;
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
    action?: string;
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
