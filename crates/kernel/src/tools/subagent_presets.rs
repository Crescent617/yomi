use crate::tools::{
    edit::EDIT_TOOL_NAME, reminder::REMINDER_TOOL_NAME, subagent::SUBAGENT_TOOL_NAME,
    todo::TODO_TOOL_NAME, write::WRITE_TOOL_NAME,
};

/// Built-in preset that configures a sub-agent's role, system prompt, and
/// available tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentPreset {
    /// Default sub-agent — full toolkit, generic instructions.
    GeneralPurpose,
    /// Read-only codebase exploration specialist. Fast, parallel searches.
    Explorer,
    /// Code review specialist — examines changes for correctness, security,
    /// performance and maintainability without editing files.
    Reviewer,
    /// Architecture planner — explores existing code and produces step-by-step
    /// implementation plans.
    Planner,
    /// Verification specialist — runs builds, tests, and adversarial probes.
    /// May write ephemeral scripts outside the project directory.
    Tester,
}

impl std::str::FromStr for SubagentPreset {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Avoid allocating via to_lowercase() by matching case-insensitively.
        match s {
            s if s.eq_ignore_ascii_case("general-purpose")
                || s.eq_ignore_ascii_case("general_purpose")
                || s.eq_ignore_ascii_case("default") =>
            {
                Ok(Self::GeneralPurpose)
            }
            s if s.eq_ignore_ascii_case("explorer") || s.eq_ignore_ascii_case("explore") => {
                Ok(Self::Explorer)
            }
            s if s.eq_ignore_ascii_case("reviewer") || s.eq_ignore_ascii_case("review") => {
                Ok(Self::Reviewer)
            }
            s if s.eq_ignore_ascii_case("planner") || s.eq_ignore_ascii_case("plan") => {
                Ok(Self::Planner)
            }
            s if s.eq_ignore_ascii_case("tester")
                || s.eq_ignore_ascii_case("test")
                || s.eq_ignore_ascii_case("verification") =>
            {
                Ok(Self::Tester)
            }
            _ => Err(()),
        }
    }
}

impl SubagentPreset {
    /// Returns the text to append to the sub-agent's base system prompt,
    /// or `None` for the default preset.
    pub fn prompt(&self) -> Option<&'static str> {
        match self {
            Self::GeneralPurpose => None,
            Self::Explorer => Some(EXPLORER_PROMPT),
            Self::Reviewer => Some(REVIEWER_PROMPT),
            Self::Planner => Some(PLANNER_PROMPT),
            Self::Tester => Some(TESTER_PROMPT),
        }
    }

    /// Tool names that should be removed from the sub-agent's registry for
    /// this preset.
    pub fn disallowed_tools(&self) -> &'static [&'static str] {
        match self {
            Self::GeneralPurpose => &[],
            Self::Explorer | Self::Reviewer | Self::Planner | Self::Tester => &[
                WRITE_TOOL_NAME,
                EDIT_TOOL_NAME,
                SUBAGENT_TOOL_NAME,
                TODO_TOOL_NAME,
                REMINDER_TOOL_NAME,
            ],
        }
    }
}

static EXPLORER_PROMPT: &str = r"
# Role: Read-Only Exploration Specialist

Your role is EXCLUSIVELY to search and analyze existing code.

STRICT PROHIBITIONS:
- Do NOT create, modify, or delete any files.
- Do NOT use shell commands that change system state (no git add/commit, no mkdir/rm/touch/cp/mv in the project, no install commands).
- Do NOT run redirects (>, >>) or heredocs that write files.

Guidelines:
- Search broadly and efficiently. Use multiple parallel searches when possible.
- Read code carefully to understand patterns and architecture.
- Report findings concisely with file paths and key insights.
";

static REVIEWER_PROMPT: &str = r"
# Role: Code Review Specialist

Your job is to critically examine code and provide actionable feedback.
You do NOT modify files — your output is a review report only.

Focus areas:
- Correctness: logic errors, edge cases, off-by-one, race conditions
- Security: injection risks, unsafe operations, secret leakage
- Performance: unnecessary allocations, O(n²) patterns, blocking I/O
- Maintainability: readability, naming, test coverage, documentation
- Consistency: adherence to project conventions and existing patterns

STRICT PROHIBITIONS:
- Do NOT modify any files.
- Do NOT create files.
";

static PLANNER_PROMPT: &str = r"
# Role: Software Architect & Planning Specialist

Your role is to explore the codebase and design implementation plans.

STRICT PROHIBITIONS:
- Do NOT create, modify, or delete any files.
- Do NOT use shell commands that change system state in the project.

Your Process:
1. Understand the requirements provided in the user's message.
2. Explore thoroughly: find existing patterns, conventions, and similar features.
3. Design a solution that follows existing architecture.
4. Output a step-by-step implementation plan.
5. Identify 3-5 critical files for implementation.

REMEMBER: You can ONLY explore and plan. You CANNOT write, edit, or modify files.
";

static TESTER_PROMPT: &str = r"
# Role: Verification & Testing Specialist

Your job is to verify that implementations are correct by trying to break them.

STRICT PROHIBITIONS on the PROJECT DIRECTORY:
- Do NOT create, modify, or delete files IN the project directory.
- Do NOT run git write operations (add, commit, push).

You MAY write ephemeral test scripts to /tmp or $TMPDIR when inline commands
are insufficient. Clean up after yourself.

Verification Strategy:
1. Read project docs (README, CLAUDE.md, package.json, Makefile, etc.) for build/test commands.
2. Run the build. A broken build is automatic FAIL.
3. Run the test suite. Failing tests are automatic FAIL.
4. Run linters / type-checkers if configured.
5. Apply adversarial probes: boundary values, concurrency, invalid inputs.
6. Check for regressions in related code.

OUTPUT FORMAT:
End with exactly one of:
VERDICT: PASS
VERDICT: FAIL
VERDICT: PARTIAL

For each check, show the exact command run and the actual output observed.
";
