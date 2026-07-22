import { formatElapsed } from "../../utils";
import { activateSession as stateActivateSession } from "../../session";

export function statusColor(status: string): string {
  switch (status) {
    case "running":
      return "text-warning border-warning/20 bg-warning/10";
    case "completed":
      return "text-success border-success/20 bg-success/10";
    case "failed":
      return "text-error border-error/20 bg-error/10";
    case "cancelled":
      return "text-muted-foreground border-muted-foreground/20 bg-muted-foreground/10";
    default:
      return "text-muted-foreground border-muted-foreground/20 bg-muted-foreground/10";
  }
}

export function compactArgs(args: string, maxLen = 120): string {
  if (!args) return "";
  try {
    const parsed = JSON.parse(args);
    const s = JSON.stringify(parsed);
    if (s.length <= maxLen) return s;
    return s.slice(0, maxLen) + "…";
  } catch {
    return (
      args.replace(/\s+/g, " ").slice(0, maxLen) +
      (args.length > maxLen ? "…" : "")
    );
  }
}

function normalizeToolName(toolName: string): string {
  const name = toolName.toLowerCase().replace(/[_-]/g, "");
  const aliases: Record<string, string> = {
    readfile: "read",
    writefile: "write",
    editfile: "edit",
    globsearch: "glob",
    grepsearch: "grep",
    bash: "shell",
    command: "shell",
    subagent: "agent",
    ask: "askuser",
    task: "todo",
    message: "postmessage",
  };
  return aliases[name] ?? name;
}

/**
 * Humanize a tool name for display: convert snake_case / kebab-case /
 * space-separated names to CamelCase (e.g. `my_custom_tool` → `MyCustomTool`).
 */
export function humanizeToolName(toolName: string): string {
  if (!toolName) return "";
  if (/^[A-Z]/.test(toolName)) return toolName;
  let result = "";
  let capitalizeNext = true;
  for (const c of toolName) {
    if (c === "_" || c === "-" || c === " ") {
      capitalizeNext = true;
    } else if (capitalizeNext) {
      result += c.toUpperCase();
      capitalizeNext = false;
    } else {
      result += c;
    }
  }
  return result;
}

export function toolLabel(toolName: string, isSubagent = false): string {
  if (isSubagent || normalizeToolName(toolName) === "agent") {
    return "Agent";
  }
  const labels: Record<string, string> = {
    read: "Read",
    write: "Write",
    edit: "Edit",
    shell: "Shell",
    bash: "Shell",
    command: "Shell",
    glob: "Glob",
    grep: "Grep",
    webfetch: "Web fetch",
    websearch: "Web search",
    skill: "Skill",
    postmessage: "Post message",
    askuser: "Ask user",
    todo: "Todo",
    reminder: "Reminder",
    sleep: "Sleep",
    updategoal: "Update goal",
    sendmessage: "Send message",
    taskcreate: "Create task",
    taskget: "Get task",
    tasklist: "List tasks",
    taskupdate: "Update task",
  };
  return (
    labels[normalizeToolName(toolName)] ??
    (humanizeToolName(toolName) || "Tool")
  );
}

function parseArgs(args: string): Record<string, unknown> | null {
  try {
    const value = JSON.parse(args);
    return value && typeof value === "object" && !Array.isArray(value)
      ? value
      : null;
  } catch {
    return null;
  }
}

function firstText(value: unknown): string {
  return typeof value === "string" ? value.replace(/\s+/g, " ").trim() : "";
}

export function extractTarget(tool_name: string, args: string): string {
  const parsed = parseArgs(args);
  if (!parsed) return "";
  const name = normalizeToolName(tool_name);
  switch (name) {
    case "read":
    case "edit":
      return firstText(parsed.path);
    case "write":
      return firstText(parsed.file_path);
    case "shell":
    case "glob":
    case "grep":
      return firstText(name === "shell" ? parsed.command : parsed.pattern);
    case "webfetch":
      return firstText(parsed.url);
    case "websearch":
      return firstText(parsed.query);
    case "skill":
      return firstText(parsed.name) || firstText(parsed.path);
    case "agent":
      return firstText(parsed.description) || firstText(parsed.prompt);
    case "postmessage":
      return firstText(parsed.agent_id);
    case "askuser": {
      const question = Array.isArray(parsed.questions)
        ? parsed.questions[0]
        : null;
      return firstText(question?.question) || firstText(question?.header);
    }
    case "todo":
      return firstText(parsed.action);
    case "reminder":
      return firstText(parsed.message);
    case "sendmessage": {
      const files = Array.isArray(parsed.files) ? parsed.files : [];
      return firstText(parsed.content) || firstText(files[0]);
    }
    case "sleep":
      return parsed.seconds == null ? "" : `${parsed.seconds}s`;
    case "updategoal":
      return firstText(parsed.status);
    case "taskcreate":
      return firstText(parsed.subject);
    case "tasklist":
      return "";
    case "taskget":
    case "taskupdate":
      return firstText(parsed.taskId) || firstText(parsed.task_id);
    default:
      return "";
  }
}

export function extraMeta(tool_name: string, args: string): string {
  const parsed = parseArgs(args);
  if (!parsed) return "";
  const name = normalizeToolName(tool_name);
  const extras: string[] = [];
  if (name === "shell") {
    if (parsed.background) extras.push("async");
    if (
      parsed.timeout != null &&
      (parsed.background || parsed.timeout !== 60)
    ) {
      extras.push(`timeout ${parsed.timeout}s`);
    }
  } else if (name === "glob") {
    if (parsed.path) extras.push(firstText(parsed.path));
  } else if (name === "grep") {
    if (parsed.output_mode && parsed.output_mode !== "filename") {
      extras.push(String(parsed.output_mode));
    }
    const scope =
      firstText(parsed.path) ||
      firstText(parsed.glob) ||
      firstText(parsed.type);
    if (scope) extras.push(scope);
    const context = parsed.context ?? parsed["-C"];
    if (context != null) extras.push(`context ${context}`);
  } else if (name === "write" && parsed.mode === "append") {
    extras.push("append");
  } else if (name === "edit" && parsed.replace_all) {
    extras.push("replace all");
  } else if (name === "websearch") {
    if (parsed.num_results != null)
      extras.push(`${parsed.num_results} results`);
  } else if (name === "askuser" && Array.isArray(parsed.questions)) {
    if (parsed.questions.length > 1) {
      extras.push(`${parsed.questions.length} questions`);
    }
  } else if (name === "todo" && Array.isArray(parsed.todos)) {
    extras.push(`${parsed.todos.length} items`);
  } else if (name === "agent" && parsed.wait_for_completion === false) {
    extras.push("async");
  } else if (name === "sendmessage" && Array.isArray(parsed.files)) {
    extras.push(`${parsed.files.length} files`);
  } else if (name === "postmessage" && parsed.title) {
    extras.push(firstText(parsed.title));
  } else if (name === "reminder" && parsed.delay_seconds != null) {
    extras.push(`${parsed.delay_seconds}s`);
  } else if (name === "tasklist" && parsed.includeCompleted) {
    extras.push("including completed");
  } else if (name === "taskupdate") {
    const update = firstText(parsed.status) || firstText(parsed.subject);
    if (update) extras.push(update);
  }
  return extras.join(" · ");
}

export interface PostMessageArgs {
  agent_id: string;
  title: string;
  content: string;
}

export function parsePostMessageArgs(args: string): PostMessageArgs | null {
  try {
    const parsed = JSON.parse(args);
    if (
      typeof parsed.agent_id === "string" &&
      typeof parsed.title === "string" &&
      typeof parsed.content === "string"
    ) {
      return parsed;
    }
  } catch {
    /* ignore */
  }
  return null;
}

export function postMessageSessionTarget(
  toolName: string,
  args: string,
): string | null {
  if (toolName.toLowerCase().replace(/[_-]/g, "") !== "postmessage")
    return null;
  return parsePostMessageArgs(args)?.agent_id || null;
}

export interface EditArgs {
  path: string;
  old_str: string;
  new_str: string;
}

export interface WriteArgs {
  file_path: string;
  content: string;
}

export function parseEditArgs(args: string): EditArgs | null {
  try {
    const parsed = JSON.parse(args);
    if (
      typeof parsed.path === "string" &&
      typeof parsed.old_str === "string" &&
      typeof parsed.new_str === "string"
    ) {
      return parsed;
    }
  } catch {
    /* ignore */
  }
  return null;
}

export function parseWriteArgs(args: string): WriteArgs | null {
  try {
    const parsed = JSON.parse(args);
    if (
      typeof parsed.file_path === "string" &&
      typeof parsed.content === "string"
    ) {
      return parsed;
    }
  } catch {
    /* ignore */
  }
  return null;
}

export function diffLines(
  oldStr: string,
  newStr: string,
): { type: "add" | "del" | "context"; text: string }[] {
  const oldLines = oldStr.split("\n");
  const newLines = newStr.split("\n");

  const dp: number[][] = Array(oldLines.length + 1)
    .fill(null)
    .map(() => Array(newLines.length + 1).fill(0));

  for (let i = 1; i <= oldLines.length; i++) {
    for (let j = 1; j <= newLines.length; j++) {
      if (oldLines[i - 1] === newLines[j - 1]) {
        dp[i][j] = dp[i - 1][j - 1] + 1;
      } else {
        dp[i][j] = Math.max(dp[i - 1][j], dp[i][j - 1]);
      }
    }
  }

  const result: { type: "add" | "del" | "context"; text: string }[] = [];
  let i = oldLines.length;
  let j = newLines.length;

  while (i > 0 || j > 0) {
    if (i > 0 && j > 0 && oldLines[i - 1] === newLines[j - 1]) {
      result.push({ type: "context", text: oldLines[i - 1] });
      i--;
      j--;
    } else if (j > 0 && (i === 0 || dp[i][j - 1] >= dp[i - 1][j])) {
      result.push({ type: "add", text: newLines[j - 1] });
      j--;
    } else {
      result.push({ type: "del", text: oldLines[i - 1] });
      i--;
    }
  }
  result.reverse();
  return result;
}

export { formatElapsed };

export async function handleJumpToSession(sessionId: string) {
  await stateActivateSession(sessionId);
}
