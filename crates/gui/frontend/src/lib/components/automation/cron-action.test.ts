import { describe, expect, it } from "vitest";
import { buildCronAction } from "./cron-action";
import type { ProjectInfo } from "../../api";

const base = {
  actionType: "send_message" as const,
  content: "  hello  ",
  command: "",
  shellWorkingDir: "",
  sessionId: "",
  project: undefined,
  selectedProjectId: undefined,
  existingTemplate: undefined,
  cwd: "/home/u/work",
};

const project: ProjectInfo = {
  id: "proj_1",
  dir: "/home/u/repo",
} as ProjectInfo;

describe("buildCronAction", () => {
  it("create per-run（无项目）：session_id 缺省，模板用 cwd 回退", () => {
    const a = buildCronAction({ ...base, useNewSession: true });
    expect(a.session_id).toBeUndefined();
    expect(a.content).toBe("hello");
    expect(a.session_template).toEqual({
      working_dir: "/home/u/work",
      project_id: undefined,
    });
  });

  it("create per-run（选中项目）：模板取项目 dir 与 id", () => {
    const a = buildCronAction({
      ...base,
      useNewSession: true,
      project,
      selectedProjectId: "proj_1",
    });
    expect(a.session_id).toBeUndefined();
    expect(a.session_template).toEqual({
      working_dir: "/home/u/repo",
      project_id: "proj_1",
    });
  });

  it("编辑已有 per-run 任务：无任何新选择时保留已有模板字段", () => {
    const a = buildCronAction({
      ...base,
      useNewSession: true,
      existingTemplate: { working_dir: "/old/dir", project_id: "proj_old" },
    });
    expect(a.session_template).toEqual({
      working_dir: "/old/dir",
      project_id: "proj_old",
    });
  });

  it("绑定固定会话：session_id 为非空值，不带模板", () => {
    const a = buildCronAction({
      ...base,
      useNewSession: false,
      sessionId: "  sess_123  ",
    });
    expect(a.session_id).toBe("sess_123");
    expect(a.session_template).toBeUndefined();
  });

  it("编辑绑定任务清空 session（转 per-run）：绑定任务无模板,回退 cwd", () => {
    const a = buildCronAction({
      ...base,
      useNewSession: false,
      sessionId: "   ",
      existingTemplate: null,
    });
    expect(a.session_id).toBeUndefined();
    expect(a.session_template).toEqual({
      working_dir: "/home/u/work",
      project_id: undefined,
    });
  });

  it("shell：只带 command/working_dir,无 session 字段", () => {
    const a = buildCronAction({
      ...base,
      actionType: "shell",
      command: "  make test  ",
      shellWorkingDir: " /tmp ",
      useNewSession: true,
    });
    expect(a).toEqual({
      type: "shell",
      command: "make test",
      working_dir: "/tmp",
    });
  });
});
