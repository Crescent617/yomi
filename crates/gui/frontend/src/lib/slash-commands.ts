// 会话级斜杠命令：主窗口输入框（ChatInput）与桌面宠物小窗输入框
// （CompanionInput）共用同一份执行逻辑，避免命令语义两处各写一份。
//
// 只放"对着某个 session 发一条命令"的部分。依赖输入框本地 state 的
// 命令通过 SessionCommandContext 回调交回调用方：/steer 要带上引号与
// 附件、/history 在主窗口是消息历史挑选器（小窗没有）。

import * as api from "./api";
import { showNotification } from "./state.svelte";
import { forkSession } from "./session";
import { askBtw } from "./btw.svelte";
import { SLASH_COMMANDS } from "./commands";
import { buildContentBlocks, type TaggedContentBlock } from "./types";

/** 命令补全用：按 "/" 之后已输入的前缀过滤共享命令表。 */
export function matchSlashCommands(
  query: string,
): readonly (readonly [string, string])[] {
  const q = query.toLowerCase();
  return SLASH_COMMANDS.filter(([cmd]) => cmd.toLowerCase().includes(q));
}

export interface SessionCommandContext {
  session_id: string;
  /** 用引号/附件包裹待发文本；缺省为原样返回。 */
  composeText?: (text: string) => string;
  /** /steer 的内容块（含内联图片）；缺省只发文本。 */
  steerBlocks?: (text: string) => TaggedContentBlock[];
  /** /steer 可带的内联图片数量，用于"空输入"判定（缺省 0）。 */
  steerImageCount?: number;
  /** /history 的处理；缺省时视作普通消息（返回 "message"）。 */
  onHistory?: () => void;
  /** /btw 的处理；缺省时走 askBtw（旁问卡片只在主窗口渲染，小窗应转提示）。 */
  onBtw?: (question: string) => void;
}

/** /debug noti：连发 10 条短命测试通知，验证通知通道。 */
async function runNotificationDebug() {
  const types = ["info", "success", "warning", "error"] as const;
  for (let index = 1; index <= 10; index += 1) {
    if (index > 1) {
      const delay = 100 + Math.floor(Math.random() * 351);
      await new Promise<void>((resolve) => window.setTimeout(resolve, delay));
    }
    const type = types[Math.floor(Math.random() * types.length)];
    showNotification(`Debug notification ${index}/10`, type);
  }
}

export type SessionCommandOutcome =
  /** 命令已执行，调用方应清空输入框。 */
  | "handled"
  /** 不是本模块认识的命令，调用方按普通消息发送。 */
  | "message"
  /** 命令被识别但用法有误或执行失败，调用方应保留草稿。 */
  | "failed";

/**
 * 执行一条以 "/" 开头的输入。匹配 `ChatInput` 原有 `handleCommand` 的
 * 行为与提示文案。
 */
export async function runSessionCommand(
  ctx: SessionCommandContext,
  text: string,
): Promise<SessionCommandOutcome> {
  const parts = text.split(/\s+/);
  const cmd = parts[0].toLowerCase();
  const session_id = ctx.session_id;
  const compose = ctx.composeText ?? ((value: string) => value);

  try {
    switch (cmd) {
      case "/debug":
        if (parts[1]?.toLowerCase() !== "noti") {
          showNotification("Unknown debug command. Try /debug noti", "warning");
          return "failed";
        }
        void runNotificationDebug();
        return "handled";
      case "/cancel":
        await api.cancelSession(session_id);
        showNotification("Session cancelled", "info");
        return "handled";
      case "/clear":
        await api.clearSession(session_id);
        showNotification("Session context cleared", "info");
        return "handled";
      case "/yolo":
        await api.setPermissionLevel(session_id, "dangerous");
        showNotification(
          "YOLO mode enabled — all tools will be auto-approved",
          "info",
        );
        return "handled";
      case "/undo": {
        const checkpoints = await api.getCheckpoints(session_id);
        if (checkpoints.length < 1) {
          showNotification("No checkpoint to undo", "error");
          return "failed";
        }
        const target = checkpoints[checkpoints.length - 1];
        await api.rewind(session_id, target.message_id);
        showNotification("Undo last turn", "info");
        return "handled";
      }
      case "/safe":
        await api.setPermissionLevel(session_id, "safe");
        showNotification("Permission level set to Safe", "info");
        return "handled";
      case "/caution":
        await api.setPermissionLevel(session_id, "caution");
        showNotification("Permission level set to Caution", "info");
        return "handled";
      case "/compact":
        await api.compactSession(session_id);
        showNotification("Session compaction requested", "info");
        return "handled";
      case "/steer": {
        const args = parts.slice(1).join(" ").trim();
        const steerText = compose(args);
        const imageCount = ctx.steerImageCount ?? 0;
        if (!steerText && imageCount === 0) {
          showNotification(
            "Please provide steer content: /steer <content>",
            "error",
          );
          return "failed";
        }
        const blocks = (
          ctx.steerBlocks ?? ((value) => buildContentBlocks(value, []))
        )(steerText);
        await api.sendSteer(session_id, blocks);
        showNotification("Steer message queued for next step", "info");
        return "handled";
      }
      case "/fork":
        try {
          const fork = await forkSession(session_id);
          showNotification(`Forked session: ${fork.id}`, "success");
        } catch (e) {
          showNotification(
            `Fork failed: ${e instanceof Error ? e.message : String(e)}`,
            "error",
          );
          return "failed";
        }
        return "handled";
      case "/continue":
        try {
          await api.continueSession(session_id);
          showNotification("Agent continuing...", "info");
        } catch (e) {
          showNotification(
            `Continue failed: ${e instanceof Error ? e.message : String(e)}`,
            "error",
          );
          return "failed";
        }
        return "handled";
      case "/history":
        if (!ctx.onHistory) return "message";
        ctx.onHistory();
        return "handled";
      case "/btw": {
        const question = parts.slice(1).join(" ").trim();
        if (!question) {
          showNotification("用法：/btw <问题>", "error");
          return "failed";
        }
        if (ctx.onBtw) {
          ctx.onBtw(question);
          return "handled";
        }
        await askBtw(session_id, question);
        return "handled";
      }
      default:
        // 不是命令——调用方按普通消息发送。
        return "message";
    }
  } catch (e: unknown) {
    const msg = api.errorMessage(e);
    console.error(`Failed to execute command ${cmd}:`, msg);
    showNotification(`Command failed: ${msg}`, "error");
    return "failed";
  }
}
