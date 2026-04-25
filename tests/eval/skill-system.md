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
- **expect**: Agent 能列出 xlsx、pptx、docx、pdf 等当前启用的内置 skill（来自轻量索引），不需要调用 skill tool 就知道；a2ui 和 ctenoctl 不应作为默认内置 skill 出现
- **anti-pattern**: Agent 说"我没有 skill 信息"或"我不知道有哪些 skill"
- **severity**: high

### [pass] Skill 按需激活
- **message**: "帮我创建一个 Excel 表格，包含产品名称和价格两列，填入 3 行示例数据"
- **expect**: Agent 自主调用 skill activate xlsx，然后按 SKILL.md 指令使用内置 XML 模板和 xlsx helper 脚本创建文件
- **anti-pattern**: Agent 不使用 skill 就直接尝试写 xlsx，使用 openpyxl 写文件，或者说不知道怎么创建 Excel
- **severity**: high

### [pending] 内置 Skill 元数据完整性
- **message**: "列出内置 skill 的 id、version、when_to_use，重点检查 docx/pdf/pptx/xlsx 是否都有这些字段。"
- **expect**: docx、pdf、pptx、xlsx 都以目录名作为稳定 id，并显示非空 version 与 when_to_use；不会把 name（minimax-docx / minimax-pdf / pptx-generator / minimax-xlsx）误当成 id
- **anti-pattern**: Office 类 skill 缺少 version 或 when_to_use，或出现 minimax-* / pptx-generator 作为可激活 id
- **severity**: medium

### [pending] 弃用内置 Skill 不主动安装
- **message**: "列出默认安装到用户目录的 builtin skill，并确认 a2ui、ctenoctl 是否存在。"
- **expect**: a2ui 和 ctenoctl 不在默认用户 skill 目录与 Available Skills 索引中；源码内仍可保留这两个目录以便以后恢复
- **anti-pattern**: 启动后自动把 a2ui 或 ctenoctl 同步到 ~/.agents/skills，或在默认索引中推荐使用它们
- **severity**: medium

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
