import { Shield, AlertTriangle, Skull } from "lucide-svelte";

export type PermissionLevel = "safe" | "caution" | "dangerous";

export function levelLabel(level: PermissionLevel): string {
  switch (level) {
    case "safe":
      return "Safe";
    case "caution":
      return "Caution";
    case "dangerous":
      return "Dangerous";
    default:
      return level;
  }
}

export function levelDescription(level: PermissionLevel): string {
  switch (level) {
    case "safe":
      return "All tools require approval";
    case "caution":
      return "Safe tools auto-approved";
    case "dangerous":
      return "Most tools auto-approved";
    default:
      return "";
  }
}

export function levelIcon(level: PermissionLevel) {
  switch (level) {
    case "safe":
      return Shield;
    case "caution":
      return AlertTriangle;
    case "dangerous":
      return Skull;
    default:
      return Shield;
  }
}

export function levelColor(level: PermissionLevel): string {
  switch (level) {
    case "safe":
      return "text-green-600 border-green-600 bg-green-600/10";
    case "caution":
      return "text-amber-600 border-amber-600 bg-amber-600/10";
    case "dangerous":
      return "text-red-600 border-red-600 bg-red-600/10";
    default:
      return "";
  }
}
