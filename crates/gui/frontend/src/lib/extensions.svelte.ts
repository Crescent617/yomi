/**
 * Module-level state shared across the Extensions panel and the Config
 * section nav. Lives outside the component so an in-flight install/remove
 * survives a section switch (the panel unmounts on switch; the daemon-side
 * work continues, but we keep the "busy" signal so the nav can refuse to
 * abandon it mid-flight).
 */
export const extensionOps = $state({
  busy: false,
});
