# Skill 系统

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-test-skill
- max-turns: 15

## setup
```bash
mkdir -p /tmp/cteno-test-skill/.cteno/skills/test-ws-skill
cat > /tmp/cteno-test-skill/.cteno/skills/test-ws-skill/SKILL.md << 'SKILLEOF'
---
name: test-ws-skill
description: A test workspace skill for QA validation
when_to_use: When testing workspace skill loading
version: 1.0.0
---

# Test Workspace Skill

This is a workspace-level skill at ${SKILL_DIR}.

Session ID: ${SESSION_ID}

Arguments: $ARGS
SKILLEOF
```

## cases

### [pass] Skill 索引注入验证
- **message**: "你有哪些可用的 skill？列出来"
- **expect**: Agent 能列出 a2ui、xlsx、pptx 等内置 skill（来自轻量索引），不需要调用 skill tool 就知道
- **anti-pattern**: Agent 说"我没有 skill 信息"或"我不知道有哪些 skill"
- **severity**: high

### [pass] Skill 按需激活
- **message**: "帮我创建一个 Excel 表格，包含产品名称和价格两列，填入 3 行示例数据"
- **expect**: Agent 自主调用 skill activate xlsx，然后按 SKILL.md 指令使用 openpyxl 创建文件
- **anti-pattern**: Agent 不使用 skill 就直接尝试写 xlsx，或者说不知道怎么创建 Excel
- **severity**: high

### [pass] 项目级 Skill 加载
- **message**: "列出所有可用的 skill，告诉我哪些是 workspace 的"
- **expect**: test-ws-skill 出现在列表中，标记为 workspace 来源
- **anti-pattern**: 只显示全局/内置 skill，忽略 /tmp/cteno-test-skill/.cteno/skills/ 下的 skill
- **severity**: high

### [pass] 变量替换验证
- **message**: "激活 test-ws-skill 这个 skill，然后告诉我它的 SKILL_DIR 替换成了什么路径"
- **expect**: 返回的指令中 ${SKILL_DIR} 已替换为 /tmp/cteno-test-skill/.cteno/skills/test-ws-skill 的绝对路径
- **anti-pattern**: 返回中包含未替换的 ${SKILL_DIR} 字面量
- **severity**: medium

### [pass] 参数传递验证
- **message**: "激活 test-ws-skill，传入参数 hello-world"
- **expect**: 返回的指令中 $ARGS 被替换为 hello-world
- **anti-pattern**: $ARGS 字面量仍然存在
- **severity**: medium
