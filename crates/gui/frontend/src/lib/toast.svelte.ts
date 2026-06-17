export interface Toast {
  id: string;
  message: string;
  type: "info" | "success" | "error" | "warning";
  duration?: number;
}

export const toasts = $state<Toast[]>([]);

let toastIdCounter = 0;

export function pushToast(
  message: string,
  type: Toast["type"] = "info",
  duration = 4000,
): string {
  const id = `toast-${++toastIdCounter}`;
  toasts.push({ id, message, type, duration });
  if (duration > 0) {
    setTimeout(() => removeToast(id), duration);
  }
  return id;
}

export function removeToast(id: string) {
  const idx = toasts.findIndex((t) => t.id === id);
  if (idx !== -1) {
    toasts.splice(idx, 1);
  }
}

export function clearToasts() {
  toasts.length = 0;
}
