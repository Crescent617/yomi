import type { ErrorMessage, Message } from "../../state.svelte";
import { findThinking, hasText } from "../../session";
import { isActivityTail } from "./activity-group";

export type DisplayItem =
  | { type: "message"; message: Message; isStreaming: boolean }
  | { type: "error_group"; messages: ErrorMessage[] }
  | {
      type: "action_group";
      messages: Message[];
      isStreaming: boolean;
      isActiveActivity: boolean;
    };

export type KeyedDisplayItem = DisplayItem & { key: string };

export interface DisplayItemSections {
  stableItems: DisplayItem[];
  dynamicItems: DisplayItem[];
  tailMessages: Message[];
}

export function buildDisplayItems(
  messages: Message[],
  streaming: boolean,
  activityActive: boolean,
): DisplayItem[] {
  const items: DisplayItem[] = [];
  let group: Message[] = [];
  let errors: ErrorMessage[] = [];

  const flushErrors = () => {
    if (errors.length > 0) {
      items.push({ type: "error_group", messages: [...errors] });
      errors = [];
    }
  };

  const flush = () => {
    if (group.length > 0) {
      const isTailGroup =
        group[group.length - 1] === messages[messages.length - 1];
      items.push({
        type: "action_group",
        messages: [...group],
        isStreaming: streaming && isTailGroup,
        isActiveActivity:
          activityActive && isTailGroup && isActivityTail(messages.at(-1)),
      });
      group = [];
    }
  };

  for (let i = 0; i < messages.length; i++) {
    const message = messages[i];
    const isLast = i === messages.length - 1;

    if (message.type === "error") {
      flush();
      errors.push(message);
      continue;
    }

    flushErrors();

    if (message.type === "user" || message.type === "steer") {
      flush();
      items.push({ type: "message", message, isStreaming: false });
      continue;
    }

    if (message.type === "tool") {
      group.push(message);
      continue;
    }

    const hasTextContent = hasText(message.content);
    const isActivity =
      findThinking(message.content) !== null ||
      Boolean(message.tool_calls?.length);

    if (isActivity) {
      group.push(message);
      if (hasTextContent) flush();
    } else {
      flush();
      items.push({
        type: "message",
        message,
        isStreaming: streaming && isLast,
      });
    }
  }

  flush();
  flushErrors();
  return items;
}

/** Whether processing this message leaves no group open for a later message. */
function isClosedAfter(message: Message): boolean {
  if (message.type === "user" || message.type === "steer") return true;
  if (message.type === "error" || message.type === "tool") return false;

  const isActivity =
    findThinking(message.content) !== null ||
    Boolean(message.tool_calls?.length);
  return !isActivity || hasText(message.content);
}

function itemBaseKey(item: DisplayItem): string {
  if (item.type === "message") {
    return `message:${item.message.type}:${item.message.id}`;
  }

  const first = item.messages[0];
  return `${item.type}:${first?.type ?? "empty"}:${first?.id ?? "empty"}`;
}

/**
 * Adds a duplicate occurrence only within the same semantic identity. Unlike an
 * absolute list index, unrelated inserts do not change existing keys.
 */
export function keyDisplayItems(items: DisplayItem[]): KeyedDisplayItem[] {
  const occurrences = new Map<string, number>();
  return items.map((item) => {
    const base = itemBaseKey(item);
    const occurrence = occurrences.get(base) ?? 0;
    occurrences.set(base, occurrence + 1);
    return { ...item, key: `${base}:${occurrence}` };
  });
}

/**
 * Incremental projection for append-only committed messages. A rewrite epoch
 * invalidates the projection. The last committed message always stays dynamic,
 * so streaming/activity flags never need to mutate sealed items.
 */
export class DisplayItemProjection {
  private epoch: string | null = null;
  private knownMessageCount = 0;
  private sealedMessageCount = 0;
  private stableIds = new Set<string>();
  private sealedItems: DisplayItem[] = [];

  update(
    sessionId: string,
    stableMessages: Message[],
    rewriteRevision: number,
    streamingMessages: Message[],
    streaming: boolean,
    activityActive: boolean,
  ): DisplayItemSections {
    const epoch = `${sessionId}:${rewriteRevision}`;
    if (
      this.epoch !== epoch ||
      stableMessages.length < this.knownMessageCount
    ) {
      this.reset(epoch, stableMessages);
    } else {
      for (let i = this.knownMessageCount; i < stableMessages.length; i++) {
        this.stableIds.add(stableMessages[i].id);
      }
      this.knownMessageCount = stableMessages.length;
    }

    let sealEnd = this.sealedMessageCount;
    // Closed committed messages are independent from the live stream and can be
    // sealed immediately, including the final committed message. Only an open
    // activity tail remains dynamic so later tool events can join its group.
    for (let i = this.sealedMessageCount; i < stableMessages.length; i++) {
      if (isClosedAfter(stableMessages[i])) sealEnd = i + 1;
    }

    if (sealEnd > this.sealedMessageCount) {
      const newlySealed = stableMessages.slice(
        this.sealedMessageCount,
        sealEnd,
      );
      this.sealedItems = [
        ...this.sealedItems,
        ...buildDisplayItems(newlySealed, false, false),
      ];
      this.sealedMessageCount = sealEnd;
    }

    const dynamicMessages = stableMessages.slice(this.sealedMessageCount);
    for (const message of streamingMessages) {
      if (!this.stableIds.has(message.id)) dynamicMessages.push(message);
    }

    return {
      stableItems: this.sealedItems,
      dynamicItems: buildDisplayItems(
        dynamicMessages,
        streaming,
        activityActive,
      ),
      tailMessages:
        dynamicMessages.length > 0
          ? dynamicMessages
          : stableMessages.length > 0
            ? [stableMessages[stableMessages.length - 1]]
            : [],
    };
  }

  private reset(epoch: string, stableMessages: Message[]) {
    this.epoch = epoch;
    this.knownMessageCount = stableMessages.length;
    this.sealedMessageCount = 0;
    this.stableIds = new Set(stableMessages.map((message) => message.id));
    this.sealedItems = [];
  }
}
