export type ActivityGroupExpansion =
  | "collapsed"
  | "expanded"
  | "latest"
  | "while_running";

export type ActivityGroupOverride = "open" | "closed" | null;

export function activityGroupExpanded(
  preference: ActivityGroupExpansion,
  isLatest: boolean,
  isActive: boolean,
  override: ActivityGroupOverride,
): boolean {
  if (override === "open") return true;
  if (override === "closed") return false;

  switch (preference) {
    case "expanded":
      return true;
    case "latest":
      return isLatest;
    case "while_running":
      return isActive;
    case "collapsed":
      return false;
  }
}
