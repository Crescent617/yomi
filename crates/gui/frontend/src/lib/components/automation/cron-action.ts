import type { CronSessionTemplate, ProjectInfo } from "../../api";

export interface CronActionInput {
  actionType: "send_message" | "shell";
  content: string;
  command: string;
  /** shell 的 working_dir 输入框值 */
  shellWorkingDir: string;
  /** true = per-run（每次运行新建独立会话）；false = 绑定固定会话 */
  useNewSession: boolean;
  /** 绑定会话输入框值（可空；编辑模式清空 = 转 per-run） */
  sessionId: string;
  /** per-run 模板：用户选中的项目（含 dir） */
  project?: ProjectInfo;
  selectedProjectId?: string;
  /** 编辑场景：任务已有的 per-run 模板（绑定的任务没有） */
  existingTemplate?: CronSessionTemplate | null;
  /** 当前工作目录：模板没有任何来源时的最终回退（undefined = 不继承） */
  cwd?: string;
}

/**
 * 构造提交给 kernel 的 cron action（JSON 形状，与 serde 对齐）。
 *
 * 语义（与 kernel 的 create/update 归一化对齐）：
 * - per-run：`session_id` 必须缺省（undefined），模板只带 cwd/project，
 *   权限由 kernel 按 config 重算钳制，调用方不传等级；
 * - 绑定：`session_id` 为非空 id，模板无意义不带；
 * - 编辑清空绑定 → per-run：保留已有模板字段，没有任何模板时用 cwd 回退，
 *   不让 per-run 会话丢工作目录继承。
 */
export function buildCronAction(
  input: CronActionInput,
): Record<string, unknown> {
  const action: Record<string, unknown> = { type: input.actionType };

  if (input.actionType === "shell") {
    action.command = input.command.trim();
    action.working_dir = input.shellWorkingDir.trim() || undefined;
    return action;
  }

  action.content = input.content.trim();

  if (input.useNewSession) {
    action.session_id = undefined;
    action.session_template = {
      working_dir:
        input.project?.dir ?? input.existingTemplate?.working_dir ?? input.cwd,
      project_id:
        (input.selectedProjectId || undefined) ??
        input.existingTemplate?.project_id ??
        undefined,
    };
    return action;
  }

  const sid = input.sessionId.trim();
  action.session_id = sid || undefined;
  action.session_template = action.session_id
    ? undefined
    : {
        working_dir: input.existingTemplate?.working_dir ?? input.cwd,
        project_id: input.existingTemplate?.project_id ?? undefined,
      };
  return action;
}
