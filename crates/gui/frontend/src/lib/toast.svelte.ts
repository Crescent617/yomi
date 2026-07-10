export interface Toast {
  id: string;
  message: string;
  type: "info" | "success" | "error" | "warning";
}

export const toasts = $state<Toast[]>([]);

const durationByType: Record<Toast["type"], number> = {
  success: 6000,
  info: 8000,
  warning: 12000,
  error: 15000,
};

let toastIdCounter = 0;

export function pushToast(
  message: string,
  type: Toast["type"] = "info",
): string {
  const id = `toast-${++toastIdCounter}`;
  toasts.push({ id, message, type });
  setTimeout(() => removeToast(id), durationByType[type]);
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
