// 斜杠命令补全的状态机与键盘导航，主窗口输入框（ChatInput）与桌面宠物
// 小窗输入框（CompanionInput）共用。检测/过滤/上下键/Tab/回车/Escape 的
// 行为与 ChatInput 原有实现一致（含"输入 / 后展示、空白或非法字符即收起"
// 的判定）。

import { matchSlashCommands } from "./slash-commands";

export interface CompletionKeyResult {
  /** 该按键是否被补全菜单消费（消费后调用方不应再当普通输入处理）。 */
  consumed: boolean;
  /** Tab/回车选中时被接受的命令（无匹配项时为 undefined）。 */
  accept?: string;
  /** Escape 关闭时的标记；调用方（小窗）据此拦截冒泡以免连带收起面板。 */
  dismissed?: boolean;
}

export function createSlashCompletion() {
  let show = $state(false);
  let filter = $state("");
  let selected = $state(0);

  const commands = $derived(matchSlashCommands(filter));

  /** 依据输入框全文更新补全状态。 */
  function detect(text: string): void {
    if (text.startsWith("/")) {
      const query = text.slice(1);
      if (/^[a-zA-Z0-9_\-:]*$/.test(query)) {
        show = true;
        filter = query;
        selected = 0;
        return;
      }
    }
    show = false;
  }

  function close(): void {
    show = false;
  }

  /** 处理一次按键；调用方据返回值决定是否继续处理该事件。 */
  function keydown(event: KeyboardEvent): CompletionKeyResult {
    if (!show) return { consumed: false };
    const list = commands;
    if (event.key === "ArrowDown") {
      event.preventDefault();
      if (list.length) selected = (selected + 1) % list.length;
      return { consumed: true };
    }
    if (event.key === "ArrowUp") {
      event.preventDefault();
      if (list.length) selected = (selected - 1 + list.length) % list.length;
      return { consumed: true };
    }
    if (event.key === "Tab" || event.key === "Enter") {
      event.preventDefault();
      const match = list[selected];
      return { consumed: true, accept: match ? match[0] : undefined };
    }
    if (event.key === "Escape") {
      show = false;
      return { consumed: true, dismissed: true };
    }
    return { consumed: false };
  }

  return {
    get show() {
      return show;
    },
    get selected() {
      return selected;
    },
    get commands() {
      return commands;
    },
    detect,
    close,
    keydown,
  };
}
