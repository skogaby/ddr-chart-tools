# Agent Conventions

Project-specific overrides to the default multi-agent workflow conventions. Read this before doing any work in this repo.

## Directory Substitution: `.spec/` replaces `.kiro/`

This project uses `.spec/` as its agent-workflow directory instead of the default `.kiro/`, for tool-agnostic interoperability across different agentic coding tools.

Wherever agent prompts, SOPs, or steering documents reference `.kiro/` for **project-scoped** paths, substitute `.spec/`:

| Default path (in agent prompts)           | This project's path                     |
|-------------------------------------------|-----------------------------------------|
| `.kiro/workspace-manifest.json`           | `.spec/workspace-manifest.json`         |
| `.kiro/steering/`                         | `.spec/steering/`                       |
| `.kiro/workflow/{feature}/`               | `.spec/workflow/{feature}/`             |
| `src/.kiro/learnings/{agent-name}.md`     | `.spec/learnings/{agent-name}.md`       |

**Global-scoped paths are NOT substituted.** Global learnings at `~/.kiro/learnings/{agent-name}.md` stay at the default location — they are shared across all workspaces and remain in `~/.kiro/` regardless of per-project conventions.

## Self-Learning

Self-learning is enabled for this project (`selfLearning.enabled: true` in `.spec/workspace-manifest.json`).

Apply the self-learning section of your agent prompt with these path overrides:

- **Project learnings** (read + write): `.spec/learnings/{agent-name}.md`
- **Global learnings** (read + write): `~/.kiro/learnings/{agent-name}.md` (unchanged)
- **Session corrections log**: `{workflowPath}/session-logs.md` (unchanged — already uses `{workflowPath}` which resolves under `.spec/workflow/`)

The `.spec/learnings/` directory will be created on first write. No setup needed.
