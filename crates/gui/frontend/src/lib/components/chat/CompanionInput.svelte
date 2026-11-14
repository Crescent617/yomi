<script lang="ts">
  // 伙伴窗 slim 输入：Enter 发送、Shift+Enter 换行、支持会话级斜杠命令。
  // 不用完整 ChatInput——附件/模型选择等 chrome 留在主窗口；命令补全只
  // 提示斜杠命令（不含 /skill:）。命令执行与补全均复用主窗口同一份实现
  // （slash-commands / slash-completion / SlashCommandMenu）。
  import { ArrowUp, Loader2 } from "lucide-svelte";
  import { showNotification } from "../../state.svelte";
  import { runSessionCommand } from "../../slash-commands";
  import { createSlashCompletion } from "../../slash-completion.svelte";
  import SlashCommandMenu from "./SlashCommandMenu.svelte";

  let {
    session_id,
    disabled,
    placeholder,
    onSend,
  }: {
    session_id: string;
    disabled: boolean;
    placeholder: string;
    onSend: (text: string) => Promise<void>;
  } = $props();

  let text = $state("");
  let sending = $state(false);
  let textarea: HTMLTextAreaElement | undefined;

  // IME：WebKit 在候选词确认那次回车上报的 isComposing 不可靠，跟主窗口
  // 一样用 composition 事件兜底——组合中忽略按键，组合结束后吞掉紧随的
  // 一次回车（100ms 内）。
  let composing = $state(false);
  let ignoreNextEnter = $state(false);

  const slashCompletion = createSlashCompletion();
  const can_send = $derived(!disabled && !sending && text.trim().length > 0);
  // 占位提示永远单行：折叠换行/连续空白（会话名可能来自渠道、未经归一），
  // 超宽由 ::placeholder 的省略号截断。
  const placeholder_text = $derived(placeholder.replace(/\s+/g, " ").trim());

  function acceptCommand(cmd: string) {
    text = cmd + " ";
    slashCompletion.close();
    textarea?.focus();
    resize();
  }

  function commandContext() {
    return {
      session_id,
      // 小窗没有消息历史挑选器（那在主窗口），如实提示而不误发原文。
      onHistory: () =>
        showNotification(
          "Message history is available in the main window",
          "info",
        ),
      // 旁问卡片只在主窗口渲染，小窗执行了也看不到结果——同样转提示。
      onBtw: () =>
        showNotification(
          "Side questions (/btw) are available in the main window",
          "info",
        ),
    };
  }

  async function submit() {
    const value = text.trim();
    if (!can_send || !value) return;
    sending = true;
    try {
      if (value.startsWith("/")) {
        const outcome = await runSessionCommand(commandContext(), value);
        if (outcome === "failed") return; // 保留草稿
        if (outcome === "message") await onSend(value);
        // "handled" → 落到下面清空
      } else {
        await onSend(value);
      }
      text = "";
      slashCompletion.close();
      resize();
    } catch {
      // 发送失败保留草稿（错误提示由 onSend 负责上报）。
    } finally {
      sending = false;
      textarea?.focus();
    }
  }

  function onkeydown(event: KeyboardEvent) {
    if (event.isComposing || composing) return;

    // 菜单没显示（无匹配）时不拦截按键——否则输入 /unknown 后回车会被
    // 静默吞掉、既不发也不提示。菜单可见时才走共享补全导航。
    if (slashCompletion.show && slashCompletion.commands.length > 0) {
      const commandKey = slashCompletion.keydown(event);
      if (commandKey.consumed) {
        if (commandKey.accept) acceptCommand(commandKey.accept);
        // Escape 只关下拉，别冒泡到宠物窗的全局处理把整个面板收起。
        if (commandKey.dismissed) {
          event.preventDefault();
          event.stopPropagation();
        }
        return;
      }
    }

    if (event.key === "Enter" && !event.shiftKey) {
      if (ignoreNextEnter) {
        ignoreNextEnter = false;
        event.preventDefault();
        return;
      }
      event.preventDefault();
      void submit();
    }
  }

  function resize() {
    if (!textarea) return;
    textarea.style.height = "auto";
    textarea.style.height = `${Math.min(textarea.scrollHeight, 120)}px`;
  }

  function handleFocusOut(event: FocusEvent) {
    const container = event.currentTarget as HTMLElement;
    if (!container.contains(event.relatedTarget as Node)) {
      slashCompletion.close();
    }
  }

  export function focus() {
    textarea?.focus();
  }
</script>

<div class="border-t border-subtle p-2" onfocusout={handleFocusOut}>
  <div class="relative">
    {#if slashCompletion.show && slashCompletion.commands.length > 0}
      <SlashCommandMenu
        commands={slashCompletion.commands}
        selected={slashCompletion.selected}
        onSelect={acceptCommand}
      />
    {/if}
    <div
      class="flex items-center gap-2 rounded-md border border-subtle bg-code-bg px-2 py-1.5 transition-shadow focus-within:ring-1 focus-within:ring-primary/50"
    >
      <textarea
        bind:this={textarea}
        bind:value={text}
        oninput={() => {
          slashCompletion.detect(text);
          resize();
        }}
        {onkeydown}
        onfocus={() => slashCompletion.detect(text)}
        oncompositionstart={() => (composing = true)}
        oncompositionend={() => {
          composing = false;
          ignoreNextEnter = true;
          setTimeout(() => (ignoreNextEnter = false), 100);
        }}
        {disabled}
        placeholder={placeholder_text}
        rows="1"
        aria-label="Companion message input"
        class="max-h-[120px] flex-1 resize-none bg-transparent text-sm outline-none placeholder:text-muted-foreground disabled:opacity-50"
      ></textarea>
      <button
        type="button"
        onclick={() => void submit()}
        disabled={!can_send}
        title="Send message"
        aria-label="Send message"
        class="rounded-full bg-primary/10 p-1.5 text-primary transition-colors hover:bg-primary/20 disabled:bg-transparent disabled:text-muted-foreground"
      >
        {#if sending}
          <Loader2 size={14} class="animate-spin motion-reduce:animate-none" />
        {:else}
          <ArrowUp size={14} />
        {/if}
      </button>
    </div>
  </div>
</div>

<style>
  /* 占位提示固定单行：长会话名不折行、超出以省略号截断。否则 WebKit 会把
     placeholder 撑成两行高度，resize() 在输入框清空那一次按 scrollHeight
     设高，就让空输入框自己长成两行。 */
  textarea::placeholder {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
