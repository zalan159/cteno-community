---
id: "skill"
name: "Skill"
description: |
  Skill runtime management: activate/deactivate skills in the current conversation,
  list installed skills, search/install from SkillHub.
  To create new skills, activate the 'skill-create' skill instead.
category: "system"
version: "2.0.0"
supports_background: false
full_disclosure: always
input_schema:
  type: object
  properties:
    operation:
      type: string
      enum: ["list", "activate", "deactivate", "search", "browse", "install"]
      description: |
        list: Show all installed skills
        activate: Load skill instructions into conversation context
        deactivate: Mark skill as no longer active
        search: Search SkillHub registry
        browse: View popular/trending skills from SkillHub
        install: Download and install from SkillHub
    id:
      type: string
      description: "Skill ID (for activate/deactivate)"
    query:
      type: string
      description: "Search query (for search operation)"
    slug:
      type: string
      description: "SkillHub skill slug (for install operation)"
    args:
      type: string
      description: "For activate: arguments passed to the skill (available as $ARGS/$1 in instructions)"
    include_resources:
      type: boolean
      description: "For activate: include file tree (default: true)"
    reason:
      type: string
      description: "For deactivate: optional reason"
    limit:
      type: number
      description: "Max results for search (default 20)"
  required:
    - operation
is_read_only: false
is_concurrency_safe: false
---

# Skill Tool

Runtime tool for skill activation and discovery.

## Quick Reference

| Operation | Required Params | Description |
|-----------|----------------|-------------|
| list | - | Show all installed skills with descriptions |
| activate | id | Load skill instructions into context |
| deactivate | id | Mark skill as inactive |
| search | query | Search SkillHub for skills |
| browse | - | View popular/trending skills |
| install | slug | Install skill from SkillHub |

## Variable Substitution

When a skill is activated, the following variables in SKILL.md are automatically replaced:
- `${SKILL_DIR}` → skill directory absolute path
- `${SESSION_ID}` → current session ID
- `${SKILL_ID}` → skill ID
- `${SKILL_NAME}` → skill display name
- `$ARGS` / `$1` → arguments passed via the `args` parameter

## Script Auto-Execution

Shell commands embedded in SKILL.md are executed at activation time:
- Fenced blocks: `` ```! command ``` `` → output replaces the block
- Inline: `` !`command` `` → output replaces inline

## Three-Layer Loading

Skills are loaded from three sources (higher priority overrides lower):
1. **builtin** — shipped with Cteno
2. **global** — `~/.agents/skills/`
3. **workspace** — `{workdir}/.cteno/skills/` (project-specific)

## Examples

### Activate a skill
```json
{"operation": "activate", "id": "xlsx"}
```

### Activate with arguments
```json
{"operation": "activate", "id": "xlsx", "args": "analyze sales data"}
```

### Search SkillHub
```json
{"operation": "search", "query": "pdf generation", "limit": 10}
```

### Install from SkillHub
```json
{"operation": "install", "slug": "pdf-generator"}
```

### Create new skill
```json
{"operation": "create", "name": "my-custom-skill"}
```
