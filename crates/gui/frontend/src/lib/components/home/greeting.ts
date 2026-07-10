// ── Home screen greeting pool ────────────────────────────────────────────
// Time-of-day aware, randomly picked on each visit to the home screen.

type TimeBucket = "morning" | "afternoon" | "evening" | "night";

function bucketOf(hour: number): TimeBucket {
  if (hour >= 5 && hour < 12) return "morning";
  if (hour >= 12 && hour < 18) return "afternoon";
  if (hour >= 18 && hour < 23) return "evening";
  return "night";
}

const COMMON: string[] = [
  "What can I help you with today?",
  "What are we building today?",
  "Ready when you are.",
  "Let's make something great.",
  "What's on your mind?",
  "Where should we start?",
  "Got a bug to squash or a feature to ship?",
  "Your codebase awaits.",
];

const BY_BUCKET: Record<TimeBucket, string[]> = {
  morning: [
    "Good morning! Fresh start, fresh commits.",
    "Morning! Coffee first, then code?",
    "Rise and compile. What's first today?",
  ],
  afternoon: [
    "Good afternoon! Let's keep the momentum going.",
    "Afternoon! What shall we tackle next?",
  ],
  evening: [
    "Good evening! One more feature before dinner?",
    "Evening session — let's make it count.",
  ],
  night: [
    "Burning the midnight oil? I'm here to help.",
    "Late night hacking — let's ship it.",
    "The best code is written after midnight. Allegedly.",
  ],
};

/** Pick a random greeting, weighted toward time-of-day specific lines. */
export function pickGreeting(now: Date = new Date()): string {
  const bucket = bucketOf(now.getHours());
  // 50/50 between time-specific and common pool
  const pool = Math.random() < 0.5 ? BY_BUCKET[bucket] : COMMON;
  return pool[Math.floor(Math.random() * pool.length)];
}
