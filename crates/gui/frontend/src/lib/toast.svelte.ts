export interface Toast {
  id: string;
  message: string;
  type: "info" | "success" | "error" | "warning";
  count: number;
  remaining_ms: number;
}

export const toasts = $state<Toast[]>([]);

const durationByType: Record<Toast["type"], number> = {
  success: 3000,
  info: 4000,
  warning: 7000,
  error: 10000,
};

interface ToastTimer {
  timeout: ReturnType<typeof setTimeout> | null;
  started_at: number;
}

const timers = new Map<string, ToastTimer>();
const maxToasts = 6;
let toastIdCounter = 0;
let paused = false;

function startTimer(toast: Toast) {
  if (paused) return;

  const timer: ToastTimer = {
    timeout: null,
    started_at: Date.now(),
  };
  timer.timeout = setTimeout(() => removeToast(toast.id), toast.remaining_ms);
  timers.set(toast.id, timer);
}

function stopTimer(toast: Toast) {
  const timer = timers.get(toast.id);
  if (!timer) return;

  if (timer.timeout !== null) clearTimeout(timer.timeout);
  toast.remaining_ms = Math.max(
    0,
    toast.remaining_ms - (Date.now() - timer.started_at),
  );
  timers.delete(toast.id);
}

export function pushToast(
  message: string,
  type: Toast["type"] = "info",
): string {
  const existingIndex = toasts.findIndex(
    (toast) => toast.type === type && toast.message === message,
  );

  if (existingIndex !== -1) {
    const existing = toasts[existingIndex];
    stopTimer(existing);
    existing.count += 1;
    existing.remaining_ms = durationByType[type];
    toasts.splice(existingIndex, 1);
    toasts.push(existing);
    startTimer(existing);
    return existing.id;
  }

  const toast: Toast = {
    id: `toast-${++toastIdCounter}`,
    message,
    type,
    count: 1,
    remaining_ms: durationByType[type],
  };
  toasts.push(toast);
  startTimer(toast);

  while (toasts.length > maxToasts) {
    removeToast(toasts[0].id);
  }

  return toast.id;
}

export function pauseToasts() {
  if (paused) return;
  paused = true;
  for (const toast of toasts) stopTimer(toast);
}

export function resumeToasts() {
  if (!paused) return;
  paused = false;
  for (const toast of [...toasts]) {
    if (toast.remaining_ms <= 0) removeToast(toast.id);
    else startTimer(toast);
  }
}

export function removeToast(id: string) {
  const toast = toasts.find((item) => item.id === id);
  if (toast) stopTimer(toast);

  const index = toasts.findIndex((item) => item.id === id);
  if (index !== -1) toasts.splice(index, 1);
}

export function clearToasts() {
  for (const timer of timers.values()) {
    if (timer.timeout !== null) clearTimeout(timer.timeout);
  }
  timers.clear();
  toasts.length = 0;
}
