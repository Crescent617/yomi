# Yomi Skills

This directory contains example skills for Yomi.

## What are Skills?

Skills are markdown files with YAML frontmatter that provide additional context and instructions to the AI. They are automatically loaded from configured folders and included in the system prompt.

## Skill Format

```markdown
---
name: skill-name
description: Short description of what this skill does
triggers:
  - keyword1
  - keyword2
---

# Skill Content

Your instructions here...
```

### Frontmatter Fields

- **name** (required): Unique identifier for the skill
- **description**: What the skill does
- **triggers**: Keywords that might activate this skill (for future use)

## Usage

To use skills, set the environment variable:

```bash
export YOMI_SKILL_FOLDERS="~/.yomi/skills,/path/to/more/skills"
```

Multiple folders can be specified, separated by commas. The tilde (`~`) is expanded to your home directory.

## Example Skills

- **rust-error-handling.md** - Guidelines for proper Rust error handling

## Creating Custom Skills

1. Create a `.md` file in your skills folder
2. Add YAML frontmatter with at least the `name` field
3. Write your instructions in markdown format
4. Set `YOMI_SKILL_FOLDERS` to include your skills folder

The skill content will be appended to the system prompt under an `## Available Skills` section.
