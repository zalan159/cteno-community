import { Metadata } from '@/sync/storageTypes';
import { ToolCall, Message } from '@/sync/typesMessage';
import { resolvePath } from '@/utils/pathUtils';
import { stringifyToolCommand } from '@/utils/toolCommand';
import * as z from 'zod';
import { Ionicons, Octicons } from '@expo/vector-icons';
import React from 'react';
import { t } from '@/text';
import { supportsHostBackgroundTransfer } from '@/utils/hostBackgroundTransfer';

// Icon factory functions
const ICON_TASK = (size: number = 24, color: string = '#000') => <Octicons name="rocket" size={size} color={color} />;
const ICON_TERMINAL = (size: number = 24, color: string = '#000') => <Octicons name="terminal" size={size} color={color} />;
const ICON_SEARCH = (size: number = 24, color: string = '#000') => <Octicons name="search" size={size} color={color} />;
const ICON_READ = (size: number = 24, color: string = '#000') => <Octicons name="eye" size={size} color={color} />;
const ICON_EDIT = (size: number = 24, color: string = '#000') => <Octicons name="file-diff" size={size} color={color} />;
const ICON_WEB = (size: number = 24, color: string = '#000') => <Ionicons name="globe-outline" size={size} color={color} />;
const ICON_EXIT = (size: number = 24, color: string = '#000') => <Ionicons name="exit-outline" size={size} color={color} />;
const ICON_TODO = (size: number = 24, color: string = '#000') => <Ionicons name="bulb-outline" size={size} color={color} />;
const ICON_REASONING = (size: number = 24, color: string = '#000') => <Octicons name="light-bulb" size={size} color={color} />;
const ICON_QUESTION = (size: number = 24, color: string = '#000') => <Ionicons name="help-circle-outline" size={size} color={color} />;
const ICON_UPLOAD = (size: number = 24, color: string = '#000') => <Ionicons name="cloud-upload-outline" size={size} color={color} />;
const ICON_SKILL = (size: number = 24, color: string = '#000') => <Ionicons name="extension-puzzle-outline" size={size} color={color} />;
const ICON_MEMORY = (size: number = 24, color: string = '#000') => <Ionicons name="library-outline" size={size} color={color} />;
const ICON_DISPATCH = (size: number = 24, color: string = '#000') => <Octicons name="workflow" size={size} color={color} />;
const ICON_IMAGE = (size: number = 24, color: string = '#000') => <Ionicons name="image-outline" size={size} color={color} />;
const ICON_BROWSER = (size: number = 24, color: string = '#000') => <Ionicons name="browsers-outline" size={size} color={color} />;
const ICON_PLAN = (size: number = 24, color: string = '#000') => <Ionicons name="map-outline" size={size} color={color} />;

// Random tip helper — returns one tip from the list with ~30% probability, null otherwise
function randomTip(tips: string[]): string | null {
    if (Math.random() > 0.3) return null;
    return tips[Math.floor(Math.random() * tips.length)];
}

// Tip pools per tool category
const TIPS = {
    memory: [
        '你可以说「请记住……」让 Agent 主动保存重要信息',
        '记忆分为私有和全局两种，私有记忆只有当前 Persona 可见',
        '每个 Persona 的 MEMORY.md 会自动加载到系统提示中',
        'Agent 会在需要时自动搜索记忆，你也可以说「回忆一下……」',
        '记忆跨会话持久保存，重启后依然有效',
    ],
    dispatch: [
        '任务图支持 DAG 依赖，上游结果会自动注入下游任务',
        '你可以为每个任务指定不同的模型，图片任务选 [视觉] 模型',
        '多个无依赖的任务会自动并行执行',
        '任务完成后结果会自动推送回来，无需手动查询',
    ],
    shell: [
        '长时间运行的命令可以点击 ↗ 转入后台，不阻塞对话',
        'Agent 执行命令前会请求你确认，你可以在设置中调整权限',
    ],
    search: [
        '搜索支持正则表达式和 glob 模式',
        'Agent 会根据需要自动搜索文件，你也可以直接让它查找',
    ],
    edit: [
        'Agent 编辑文件前会先读取内容，确保精确修改',
        '你可以让 Agent 同时修改多个文件，它会逐一处理',
    ],
    web: [
        '可以让 Agent 搜索网页或抓取特定 URL 的内容',
        'Agent 可以抓取网页并根据你的问题提取关键信息',
    ],
    skill: [
        'Skill 是可复用的指导模块，帮助 Agent 完成特定领域的任务',
        '你可以从 GitHub 安装社区技能，或自己编写 SKILL.md',
    ],
    schedule: [
        '定时任务支持 cron 表达式和自然语言描述',
        '你可以说「每天早上 9 点帮我……」来创建定时任务',
    ],
    image: [
        '文生图工具会调用 AI 模型生成图片',
        '你可以描述想要的画面风格、构图和细节',
    ],
    plan: [
        'Agent 会先制定计划再执行，确保方向正确',
        '你可以在计划阶段提出修改意见，避免返工',
    ],
};

const taskLikeTool = {
    title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
        // Check for description field at runtime
        if (opts.tool.input && opts.tool.input.description && typeof opts.tool.input.description === 'string') {
            return opts.tool.input.description;
        }
        return t('tools.names.task');
    },
    icon: ICON_TASK,
    isMutable: true,
    minimal: (opts: { metadata: Metadata | null, tool: ToolCall, messages?: Message[] }) => {
        // Check if there would be any filtered tasks
        const messages = opts.messages || [];
        for (let m of messages) {
            if (m.kind === 'tool-call' &&
                (m.tool.state === 'running' || m.tool.state === 'completed' || m.tool.state === 'error')) {
                return false; // Has active sub-tasks, show expanded
            }
        }
        return true; // No active sub-tasks, render as minimal
    },
    input: z.object({
        prompt: z.string().describe('The task for the agent to perform'),
        subagent_type: z.string().optional().describe('The type of specialized agent to use')
    }).partial().passthrough()
};

export const knownTools = {
    'Task': taskLikeTool,
    'Agent': taskLikeTool,
    'Bash': {
        title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (opts.tool.description) {
                return opts.tool.description;
            }
            return t('tools.names.terminal');
        },
        icon: ICON_TERMINAL,
        minimal: true,
        hideDefaultError: true,
        isMutable: true,
        input: z.object({
            command: z.string().describe('The command to execute'),
            timeout: z.number().optional().describe('Timeout in milliseconds (max 600000)')
        }),
        result: z.object({
            stderr: z.string(),
            stdout: z.string(),
        }).partial().passthrough(),
        extractDescription: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (typeof opts.tool.input.command === 'string') {
                const cmd = opts.tool.input.command;
                // Extract just the command name for common commands
                const firstWord = cmd.split(' ')[0];
                if (['cd', 'ls', 'pwd', 'mkdir', 'rm', 'cp', 'mv', 'npm', 'yarn', 'git'].includes(firstWord)) {
                    return t('tools.desc.terminalCmd', { cmd: firstWord });
                }
                // For other commands, show truncated version
                const truncated = cmd.length > 20 ? cmd.substring(0, 20) + '...' : cmd;
                return t('tools.desc.terminalCmd', { cmd: truncated });
            }
            return t('tools.names.terminal');
        },
        extractSubtitle: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (typeof opts.tool.input.command === 'string') {
                return opts.tool.input.command;
            }
            return null;
        },
        extractTip: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (supportsHostBackgroundTransfer(opts.metadata, opts.tool)) {
                return '点击 ↗ 可将命令转入后台继续执行，不阻塞对话';
            }
            return randomTip(TIPS.shell);
        }
    },
    'Glob': {
        title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (typeof opts.tool.input.pattern === 'string') {
                return opts.tool.input.pattern;
            }
            return t('tools.names.searchFiles');
        },
        icon: ICON_SEARCH,
        minimal: true,
        input: z.object({
            pattern: z.string().describe('The glob pattern to match files against'),
            path: z.string().optional().describe('The directory to search in')
        }).partial().passthrough(),
        extractDescription: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (typeof opts.tool.input.pattern === 'string') {
                return t('tools.desc.searchPattern', { pattern: opts.tool.input.pattern });
            }
            return t('tools.names.search');
        },
        extractTip: () => randomTip(TIPS.search),
    },
    'Grep': {
        title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (typeof opts.tool.input.pattern === 'string') {
                return `grep(pattern: ${opts.tool.input.pattern})`;
            }
            return 'Search Content';
        },
        icon: ICON_READ,
        minimal: true,
        input: z.object({
            pattern: z.string().describe('The regular expression pattern to search for'),
            path: z.string().optional().describe('File or directory to search in'),
            output_mode: z.enum(['content', 'files_with_matches', 'count']).optional(),
            '-n': z.boolean().optional().describe('Show line numbers'),
            '-i': z.boolean().optional().describe('Case insensitive search'),
            '-A': z.number().optional().describe('Lines to show after match'),
            '-B': z.number().optional().describe('Lines to show before match'),
            '-C': z.number().optional().describe('Lines to show before and after match'),
            glob: z.string().optional().describe('Glob pattern to filter files'),
            type: z.string().optional().describe('File type to search'),
            head_limit: z.number().optional().describe('Limit output to first N lines/entries'),
            multiline: z.boolean().optional().describe('Enable multiline mode')
        }).partial().passthrough(),
        extractDescription: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (typeof opts.tool.input.pattern === 'string') {
                const pattern = opts.tool.input.pattern.length > 20
                    ? opts.tool.input.pattern.substring(0, 20) + '...'
                    : opts.tool.input.pattern;
                return `Search(pattern: ${pattern})`;
            }
            return 'Search';
        },
        extractTip: () => randomTip(TIPS.search),
    },
    'LS': {
        title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (typeof opts.tool.input.path === 'string') {
                return resolvePath(opts.tool.input.path, opts.metadata);
            }
            return t('tools.names.listFiles');
        },
        icon: ICON_SEARCH,
        minimal: true,
        input: z.object({
            path: z.string().describe('The absolute path to the directory to list'),
            ignore: z.array(z.string()).optional().describe('List of glob patterns to ignore')
        }).partial().passthrough(),
        extractDescription: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (typeof opts.tool.input.path === 'string') {
                const path = resolvePath(opts.tool.input.path, opts.metadata);
                const basename = path.split('/').pop() || path;
                return t('tools.desc.searchPath', { basename });
            }
            return t('tools.names.search');
        }
    },
    'ExitPlanMode': {
        title: t('tools.names.planProposal'),
        icon: ICON_EXIT,
        input: z.object({
            plan: z.string().describe('The plan you came up with')
        }).partial().passthrough()
    },
    'exit_plan_mode': {
        title: t('tools.names.planProposal'),
        icon: ICON_EXIT,
        input: z.object({
            plan: z.string().describe('The plan you came up with')
        }).partial().passthrough()
    },
    'Read': {
        title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (typeof opts.tool.input.file_path === 'string') {
                const path = resolvePath(opts.tool.input.file_path, opts.metadata);
                return path;
            }
            // Gemini uses 'locations' array with 'path' field
            if (opts.tool.input.locations && Array.isArray(opts.tool.input.locations) && opts.tool.input.locations[0]?.path) {
                const path = resolvePath(opts.tool.input.locations[0].path, opts.metadata);
                return path;
            }
            return t('tools.names.readFile');
        },
        minimal: true,
        icon: ICON_READ,
        input: z.object({
            file_path: z.string().describe('The absolute path to the file to read'),
            limit: z.number().optional().describe('The number of lines to read'),
            offset: z.number().optional().describe('The line number to start reading from'),
            // Gemini format
            items: z.array(z.any()).optional(),
            locations: z.array(z.object({ path: z.string() }).passthrough()).optional()
        }).partial().passthrough(),
        result: z.object({
            file: z.object({
                filePath: z.string().describe('The absolute path to the file to read'),
                content: z.string().describe('The content of the file'),
                numLines: z.number().describe('The number of lines in the file'),
                startLine: z.number().describe('The line number to start reading from'),
                totalLines: z.number().describe('The total number of lines in the file')
            }).passthrough().optional()
        }).partial().passthrough()
    },
    // Gemini uses lowercase 'read'
    'read': {
        title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            // Gemini uses 'locations' array with 'path' field
            if (opts.tool.input.locations && Array.isArray(opts.tool.input.locations) && opts.tool.input.locations[0]?.path) {
                const path = resolvePath(opts.tool.input.locations[0].path, opts.metadata);
                return path;
            }
            if (typeof opts.tool.input.file_path === 'string') {
                const path = resolvePath(opts.tool.input.file_path, opts.metadata);
                return path;
            }
            return t('tools.names.readFile');
        },
        minimal: true,
        icon: ICON_READ,
        input: z.object({
            items: z.array(z.any()).optional(),
            locations: z.array(z.object({ path: z.string() }).passthrough()).optional(),
            file_path: z.string().optional()
        }).partial().passthrough()
    },
    'Edit': {
        title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (typeof opts.tool.input.file_path === 'string') {
                const path = resolvePath(opts.tool.input.file_path, opts.metadata);
                return path;
            }
            return t('tools.names.editFile');
        },
        icon: ICON_EDIT,
        isMutable: true,
        input: z.object({
            file_path: z.string().describe('The absolute path to the file to modify'),
            old_string: z.string().describe('The text to replace'),
            new_string: z.string().describe('The text to replace it with'),
            replace_all: z.boolean().optional().default(false).describe('Replace all occurrences')
        }).partial().passthrough(),
        extractTip: () => randomTip(TIPS.edit),
    },
    'MultiEdit': {
        title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (typeof opts.tool.input.file_path === 'string') {
                const path = resolvePath(opts.tool.input.file_path, opts.metadata);
                const editCount = Array.isArray(opts.tool.input.edits) ? opts.tool.input.edits.length : 0;
                if (editCount > 1) {
                    return t('tools.desc.multiEditEdits', { path, count: editCount });
                }
                return path;
            }
            return t('tools.names.editFile');
        },
        icon: ICON_EDIT,
        isMutable: true,
        input: z.object({
            file_path: z.string().describe('The absolute path to the file to modify'),
            edits: z.array(z.object({
                old_string: z.string().describe('The text to replace'),
                new_string: z.string().describe('The text to replace it with'),
                replace_all: z.boolean().optional().default(false).describe('Replace all occurrences')
            })).describe('Array of edit operations')
        }).partial().passthrough(),
        extractStatus: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (typeof opts.tool.input.file_path === 'string') {
                const path = resolvePath(opts.tool.input.file_path, opts.metadata);
                const editCount = Array.isArray(opts.tool.input.edits) ? opts.tool.input.edits.length : 0;
                if (editCount > 0) {
                    return t('tools.desc.multiEditEdits', { path, count: editCount });
                }
                return path;
            }
            return null;
        },
        extractTip: () => randomTip(TIPS.edit),
    },
    'Write': {
        title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (typeof opts.tool.input.file_path === 'string') {
                const path = resolvePath(opts.tool.input.file_path, opts.metadata);
                return path;
            }
            return t('tools.names.writeFile');
        },
        icon: ICON_EDIT,
        isMutable: true,
        input: z.object({
            file_path: z.string().describe('The absolute path to the file to write'),
            content: z.string().describe('The content to write to the file')
        }).partial().passthrough(),
        extractTip: () => randomTip(TIPS.edit),
    },
    'WebFetch': {
        title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (typeof opts.tool.input.url === 'string') {
                try {
                    const url = new URL(opts.tool.input.url);
                    return url.hostname;
                } catch {
                    return t('tools.names.fetchUrl');
                }
            }
            return t('tools.names.fetchUrl');
        },
        icon: ICON_WEB,
        minimal: true,
        input: z.object({
            url: z.string().url().describe('The URL to fetch content from'),
            prompt: z.string().describe('The prompt to run on the fetched content')
        }).partial().passthrough(),
        extractDescription: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (typeof opts.tool.input.url === 'string') {
                try {
                    const url = new URL(opts.tool.input.url);
                    return t('tools.desc.fetchUrlHost', { host: url.hostname });
                } catch {
                    return t('tools.names.fetchUrl');
                }
            }
            return 'Fetch URL';
        },
        extractTip: () => randomTip(TIPS.web),
    },
    'fetch': {
        title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (typeof opts.tool.input.url === 'string') {
                try {
                    const url = new URL(opts.tool.input.url);
                    return url.hostname;
                } catch {
                    return 'Fetch';
                }
            }
            return 'Fetch';
        },
        icon: ICON_WEB,
        input: z.object({
            url: z.string().describe('The URL to fetch'),
            prompt: z.string().describe('User question to guide extraction'),
            max_length: z.number().optional(),
            raw: z.boolean().optional()
        }).partial().passthrough(),
        extractDescription: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (typeof opts.tool.input.prompt === 'string') {
                const prompt = opts.tool.input.prompt;
                return prompt.length > 50 ? prompt.substring(0, 50) + '...' : prompt;
            }
            if (typeof opts.tool.input.url === 'string') {
                try {
                    return new URL(opts.tool.input.url).hostname;
                } catch { /* ignore */ }
            }
            return 'Fetch URL';
        },
        extractTip: () => randomTip(TIPS.web),
    },
    'NotebookRead': {
        title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (typeof opts.tool.input.notebook_path === 'string') {
                const path = resolvePath(opts.tool.input.notebook_path, opts.metadata);
                return path;
            }
            return t('tools.names.readNotebook');
        },
        icon: ICON_READ,
        minimal: true,
        input: z.object({
            notebook_path: z.string().describe('The absolute path to the Jupyter notebook file'),
            cell_id: z.string().optional().describe('The ID of a specific cell to read')
        }).partial().passthrough()
    },
    'NotebookEdit': {
        title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (typeof opts.tool.input.notebook_path === 'string') {
                const path = resolvePath(opts.tool.input.notebook_path, opts.metadata);
                return path;
            }
            return t('tools.names.editNotebook');
        },
        icon: ICON_EDIT,
        isMutable: true,
        input: z.object({
            notebook_path: z.string().describe('The absolute path to the notebook file'),
            new_source: z.string().describe('The new source for the cell'),
            cell_id: z.string().optional().describe('The ID of the cell to edit'),
            cell_type: z.enum(['code', 'markdown']).optional().describe('The type of the cell'),
            edit_mode: z.enum(['replace', 'insert', 'delete']).optional().describe('The type of edit to make')
        }).partial().passthrough(),
        extractDescription: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (typeof opts.tool.input.notebook_path === 'string') {
                const path = resolvePath(opts.tool.input.notebook_path, opts.metadata);
                const mode = opts.tool.input.edit_mode || 'replace';
                return t('tools.desc.editNotebookMode', { path, mode });
            }
            return t('tools.names.editNotebook');
        }
    },
    'TodoWrite': {
        title: t('tools.names.todoList'),
        icon: ICON_TODO,
        noStatus: true,
        minimal: (opts: { metadata: Metadata | null, tool: ToolCall, messages?: Message[] }) => {
            // Check if there are todos in the input
            if (opts.tool.input?.todos && Array.isArray(opts.tool.input.todos) && opts.tool.input.todos.length > 0) {
                return false; // Has todos, show expanded
            }
            
            // Check if there are todos in the result
            if (opts.tool.result?.newTodos && Array.isArray(opts.tool.result.newTodos) && opts.tool.result.newTodos.length > 0) {
                return false; // Has todos, show expanded
            }
            
            return true; // No todos, render as minimal
        },
        input: z.object({
            todos: z.array(z.object({
                content: z.string().describe('The todo item content'),
                status: z.enum(['pending', 'in_progress', 'completed']).describe('The status of the todo'),
                priority: z.enum(['high', 'medium', 'low']).optional().describe('The priority of the todo'),
                id: z.string().optional().describe('Unique identifier for the todo')
            }).passthrough()).describe('The updated todo list')
        }).partial().passthrough(),
        result: z.object({
            oldTodos: z.array(z.object({
                content: z.string().describe('The todo item content'),
                status: z.enum(['pending', 'in_progress', 'completed']).describe('The status of the todo'),
                priority: z.enum(['high', 'medium', 'low']).optional().describe('The priority of the todo'),
                id: z.string().describe('Unique identifier for the todo')
            }).passthrough()).describe('The old todo list'),
            newTodos: z.array(z.object({
                content: z.string().describe('The todo item content'),
                status: z.enum(['pending', 'in_progress', 'completed']).describe('The status of the todo'),
                priority: z.enum(['high', 'medium', 'low']).optional().describe('The priority of the todo'),
                id: z.string().describe('Unique identifier for the todo')
            }).passthrough()).describe('The new todo list')
        }).partial().passthrough(),
        extractDescription: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (Array.isArray(opts.tool.input.todos)) {
                const count = opts.tool.input.todos.length;
                return t('tools.desc.todoListCount', { count });
            }
            return t('tools.names.todoList');
        },
    },
    'WebSearch': {
        title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (typeof opts.tool.input.query === 'string') {
                return opts.tool.input.query;
            }
            return t('tools.names.webSearch');
        },
        icon: ICON_WEB,
        minimal: true,
        input: z.object({
            query: z.string().min(2).describe('The search query to use'),
            allowed_domains: z.array(z.string()).optional().describe('Only include results from these domains'),
            blocked_domains: z.array(z.string()).optional().describe('Never include results from these domains')
        }).partial().passthrough(),
        extractDescription: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (typeof opts.tool.input.query === 'string') {
                const query = opts.tool.input.query.length > 30
                    ? opts.tool.input.query.substring(0, 30) + '...'
                    : opts.tool.input.query;
                return t('tools.desc.webSearchQuery', { query });
            }
            return t('tools.names.webSearch');
        },
        extractTip: () => randomTip(TIPS.web),
    },
    'CodexBash': {
        title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            // Check if this is a single read command
            if (opts.tool.input?.parsed_cmd && 
                Array.isArray(opts.tool.input.parsed_cmd) && 
                opts.tool.input.parsed_cmd.length === 1 && 
                opts.tool.input.parsed_cmd[0].type === 'read' &&
                opts.tool.input.parsed_cmd[0].name) {
                // Display the file name being read
                const path = resolvePath(opts.tool.input.parsed_cmd[0].name, opts.metadata);
                return path;
            }
            return t('tools.names.terminal');
        },
        icon: ICON_TERMINAL,
        minimal: true,
        hideDefaultError: true,
        isMutable: true,
        input: z.object({
            command: z.array(z.string()).describe('The command array to execute'),
            cwd: z.string().optional().describe('Current working directory'),
            parsed_cmd: z.array(z.object({
                type: z.string().describe('Type of parsed command (read, write, bash, etc.)'),
                cmd: z.string().optional().describe('The command string'),
                name: z.string().optional().describe('File name or resource name')
            }).passthrough()).optional().describe('Parsed command information')
        }).partial().passthrough(),
        extractSubtitle: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            // For single read commands, show the actual command
            if (opts.tool.input?.parsed_cmd && 
                Array.isArray(opts.tool.input.parsed_cmd) && 
                opts.tool.input.parsed_cmd.length === 1 &&
                opts.tool.input.parsed_cmd[0].type === 'read') {
                const parsedCmd = opts.tool.input.parsed_cmd[0];
                if (parsedCmd.cmd) {
                    // Show the command but truncate if too long
                    const cmd = parsedCmd.cmd;
                    return cmd.length > 50 ? cmd.substring(0, 50) + '...' : cmd;
                }
            }
            // Show the actual command being executed for other cases
            if (opts.tool.input?.parsed_cmd && Array.isArray(opts.tool.input.parsed_cmd) && opts.tool.input.parsed_cmd.length > 0) {
                const parsedCmd = opts.tool.input.parsed_cmd[0];
                if (parsedCmd.cmd) {
                    return parsedCmd.cmd;
                }
            }
            return stringifyToolCommand(opts.tool.input?.command);
        },
        extractDescription: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            // Provide a description based on the parsed command type
            if (opts.tool.input?.parsed_cmd &&
                Array.isArray(opts.tool.input.parsed_cmd) &&
                opts.tool.input.parsed_cmd.length === 1) {
                const parsedCmd = opts.tool.input.parsed_cmd[0];
                if (parsedCmd.type === 'read' && parsedCmd.name) {
                    // For single read commands, show "Reading" as simple description
                    // The file path is already in the title
                    const path = resolvePath(parsedCmd.name, opts.metadata);
                    const basename = path.split('/').pop() || path;
                    return t('tools.desc.readingFile', { file: basename });
                } else if (parsedCmd.type === 'write' && parsedCmd.name) {
                    const path = resolvePath(parsedCmd.name, opts.metadata);
                    const basename = path.split('/').pop() || path;
                    return t('tools.desc.writingFile', { file: basename });
                }
            }
            return t('tools.names.terminal');
        }
    },
    'CodexReasoning': {
        title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            // Use the title from input if provided
            if (opts.tool.input?.title && typeof opts.tool.input.title === 'string') {
                return opts.tool.input.title;
            }
            return t('tools.names.reasoning');
        },
        icon: ICON_REASONING,
        minimal: true,
        input: z.object({
            title: z.string().describe('The title of the reasoning')
        }).partial().passthrough(),
        result: z.object({
            content: z.string().describe('The reasoning content'),
            status: z.enum(['completed', 'in_progress', 'error']).optional().describe('The status of the reasoning')
        }).partial().passthrough(),
        extractDescription: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (opts.tool.input?.title && typeof opts.tool.input.title === 'string') {
                return opts.tool.input.title;
            }
            return t('tools.names.reasoning');
        }
    },
    'GeminiReasoning': {
        title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            // Use the title from input if provided
            if (opts.tool.input?.title && typeof opts.tool.input.title === 'string') {
                return opts.tool.input.title;
            }
            return t('tools.names.reasoning');
        },
        icon: ICON_REASONING,
        minimal: true,
        input: z.object({
            title: z.string().describe('The title of the reasoning')
        }).partial().passthrough(),
        result: z.object({
            content: z.string().describe('The reasoning content'),
            status: z.enum(['completed', 'in_progress', 'canceled']).optional().describe('The status of the reasoning')
        }).partial().passthrough(),
        extractDescription: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (opts.tool.input?.title && typeof opts.tool.input.title === 'string') {
                return opts.tool.input.title;
            }
            return t('tools.names.reasoning');
        }
    },
    'think': {
        title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            // Use the title from input if provided
            if (opts.tool.input?.title && typeof opts.tool.input.title === 'string') {
                return opts.tool.input.title;
            }
            return t('tools.names.reasoning');
        },
        icon: ICON_REASONING,
        minimal: true,
        input: z.object({
            title: z.string().optional().describe('The title of the thinking'),
            items: z.array(z.any()).optional().describe('Items to think about'),
            locations: z.array(z.any()).optional().describe('Locations to consider')
        }).partial().passthrough(),
        result: z.object({
            content: z.string().optional().describe('The reasoning content'),
            text: z.string().optional().describe('The reasoning text'),
            status: z.enum(['completed', 'in_progress', 'canceled']).optional().describe('The status')
        }).partial().passthrough(),
        extractDescription: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (opts.tool.input?.title && typeof opts.tool.input.title === 'string') {
                return opts.tool.input.title;
            }
            return t('tools.names.reasoning');
        }
    },
    'change_title': {
        title: 'Change Title',
        icon: ICON_EDIT,
        minimal: true,
        noStatus: true,
        input: z.object({
            title: z.string().optional().describe('New session title')
        }).partial().passthrough(),
        result: z.object({}).partial().passthrough()
    },
    // Gemini internal tools - should be hidden (minimal)
    'search': {
        title: t('tools.names.search'),
        icon: ICON_SEARCH,
        minimal: true,
        input: z.object({
            items: z.array(z.any()).optional(),
            locations: z.array(z.any()).optional()
        }).partial().passthrough()
    },
    'edit': {
        title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            // Gemini sends data in nested structure, try multiple locations
            let filePath: string | undefined;
            
            // 1. Check toolCall.content[0].path
            if (opts.tool.input?.toolCall?.content?.[0]?.path) {
                filePath = opts.tool.input.toolCall.content[0].path;
            }
            // 2. Check toolCall.title (has nice "Writing to ..." format)
            else if (opts.tool.input?.toolCall?.title) {
                return opts.tool.input.toolCall.title;
            }
            // 3. Check input[0].path (array format)
            else if (Array.isArray(opts.tool.input?.input) && opts.tool.input.input[0]?.path) {
                filePath = opts.tool.input.input[0].path;
            }
            // 4. Check direct path field
            else if (typeof opts.tool.input?.path === 'string') {
                filePath = opts.tool.input.path;
            }
            
            if (filePath) {
                return resolvePath(filePath, opts.metadata);
            }
            return t('tools.names.editFile');
        },
        icon: ICON_EDIT,
        isMutable: true,
        input: z.object({
            path: z.string().describe('The file path to edit'),
            oldText: z.string().describe('The text to replace'),
            newText: z.string().describe('The new text'),
            type: z.string().optional().describe('Type of edit (diff)')
        }).partial().passthrough()
    },
    'shell': {
        title: t('tools.names.terminal'),
        icon: ICON_TERMINAL,
        minimal: true,
        isMutable: true,
        input: z.object({}).partial().passthrough(),
        extractTip: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (supportsHostBackgroundTransfer(opts.metadata, opts.tool)) {
                return '点击 ↗ 可将命令转入后台继续执行，不阻塞对话';
            }
            return randomTip(TIPS.shell);
        }
    },
    'execute': {
        title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            // Gemini sends nice title in toolCall.title
            if (opts.tool.input?.toolCall?.title) {
                // Title is like "rm file.txt [cwd /path] (description)"
                // Extract just the command part before [
                const fullTitle = opts.tool.input.toolCall.title;
                const bracketIdx = fullTitle.indexOf(' [');
                if (bracketIdx > 0) {
                    return fullTitle.substring(0, bracketIdx);
                }
                return fullTitle;
            }
            return t('tools.names.terminal');
        },
        icon: ICON_TERMINAL,
        isMutable: true,
        input: z.object({}).partial().passthrough(),
        extractSubtitle: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            // Extract description from parentheses at the end
            if (opts.tool.input?.toolCall?.title) {
                const title = opts.tool.input.toolCall.title;
                const parenMatch = title.match(/\(([^)]+)\)$/);
                if (parenMatch) {
                    return parenMatch[1];
                }
            }
            return null;
        }
    },
    'CodexPatch': {
        title: t('tools.names.applyChanges'),
        icon: ICON_EDIT,
        minimal: false,  // Show full patch rendering
        hideDefaultError: true,
        isMutable: true,
        input: z.object({
            auto_approved: z.boolean().optional().describe('Whether changes were auto-approved'),
            changes: z.record(z.string(), z.object({
                diff: z.string().optional().describe('Unified diff for the change'),
                kind: z.object({
                    type: z.string().optional().describe("add | delete | update"),
                    move_path: z.string().nullish().describe('Destination path for renames')
                }).partial().passthrough().optional(),
                add: z.object({
                    content: z.string()
                }).optional(),
                modify: z.object({
                    old_content: z.string(),
                    new_content: z.string()
                }).optional(),
                delete: z.object({
                    content: z.string()
                }).optional()
            }).passthrough()).optional().describe('File changes keyed by path'),
            fileChanges: z.record(z.string(), z.any()).optional().describe('Legacy shape for changes')
        }).partial().passthrough(),
        extractSubtitle: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            // Show the first file being modified (accepts `changes` or `fileChanges`)
            const rec = (opts.tool.input?.changes && typeof opts.tool.input.changes === 'object' && !Array.isArray(opts.tool.input.changes))
                ? opts.tool.input.changes
                : (opts.tool.input?.fileChanges && typeof opts.tool.input.fileChanges === 'object' && !Array.isArray(opts.tool.input.fileChanges))
                    ? opts.tool.input.fileChanges
                    : null;
            if (rec) {
                const files = Object.keys(rec);
                if (files.length > 0) {
                    const path = resolvePath(files[0], opts.metadata);
                    const fileName = path.split('/').pop() || path;
                    if (files.length > 1) {
                        return t('tools.desc.modifyingMultipleFiles', {
                            file: fileName,
                            count: files.length - 1
                        });
                    }
                    return fileName;
                }
            }
            return null;
        },
        extractDescription: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            // Show the number of files being modified
            const rec = (opts.tool.input?.changes && typeof opts.tool.input.changes === 'object' && !Array.isArray(opts.tool.input.changes))
                ? opts.tool.input.changes
                : (opts.tool.input?.fileChanges && typeof opts.tool.input.fileChanges === 'object' && !Array.isArray(opts.tool.input.fileChanges))
                    ? opts.tool.input.fileChanges
                    : null;
            if (rec) {
                const files = Object.keys(rec);
                const fileCount = files.length;
                if (fileCount === 1) {
                    const path = resolvePath(files[0], opts.metadata);
                    const fileName = path.split('/').pop() || path;
                    return t('tools.desc.modifyingFile', { file: fileName });
                } else if (fileCount > 1) {
                    return t('tools.desc.modifyingFiles', { count: fileCount });
                }
            }
            return t('tools.names.applyChanges');
        }
    },
    'GeminiBash': {
        title: t('tools.names.terminal'),
        icon: ICON_TERMINAL,
        minimal: true,
        hideDefaultError: true,
        isMutable: true,
        input: z.object({
            command: z.array(z.string()).describe('The command array to execute'),
            cwd: z.string().optional().describe('Current working directory')
        }).partial().passthrough(),
        extractSubtitle: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            return stringifyToolCommand(opts.tool.input?.command);
        }
    },
    'GeminiPatch': {
        title: t('tools.names.applyChanges'),
        icon: ICON_EDIT,
        minimal: true,
        hideDefaultError: true,
        isMutable: true,
        input: z.object({
            auto_approved: z.boolean().optional().describe('Whether changes were auto-approved'),
            changes: z.record(z.string(), z.object({
                add: z.object({
                    content: z.string()
                }).optional(),
                modify: z.object({
                    old_content: z.string(),
                    new_content: z.string()
                }).optional(),
                delete: z.object({
                    content: z.string()
                }).optional()
            }).passthrough()).describe('File changes to apply')
        }).partial().passthrough(),
        extractSubtitle: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            // Show the first file being modified
            if (opts.tool.input?.changes && typeof opts.tool.input.changes === 'object') {
                const files = Object.keys(opts.tool.input.changes);
                if (files.length > 0) {
                    const path = resolvePath(files[0], opts.metadata);
                    const fileName = path.split('/').pop() || path;
                    if (files.length > 1) {
                        return t('tools.desc.modifyingMultipleFiles', { 
                            file: fileName, 
                            count: files.length - 1 
                        });
                    }
                    return fileName;
                }
            }
            return null;
        },
        extractDescription: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            // Show the number of files being modified
            if (opts.tool.input?.changes && typeof opts.tool.input.changes === 'object') {
                const files = Object.keys(opts.tool.input.changes);
                const fileCount = files.length;
                if (fileCount === 1) {
                    const path = resolvePath(files[0], opts.metadata);
                    const fileName = path.split('/').pop() || path;
                    return t('tools.desc.modifyingFile', { file: fileName });
                } else if (fileCount > 1) {
                    return t('tools.desc.modifyingFiles', { count: fileCount });
                }
            }
            return t('tools.names.applyChanges');
        }
    },
    'CodexDiff': {
        title: t('tools.names.viewDiff'),
        icon: ICON_EDIT,
        minimal: false,  // Show full diff view
        hideDefaultError: true,
        noStatus: true,  // Always successful, stateless like Task
        input: z.object({
            unified_diff: z.string().describe('Unified diff content')
        }).partial().passthrough(),
        result: z.object({
            status: z.literal('completed').describe('Always completed')
        }).partial().passthrough(),
        extractSubtitle: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            // Try to extract filename from unified diff
            if (opts.tool.input?.unified_diff && typeof opts.tool.input.unified_diff === 'string') {
                const diffLines = opts.tool.input.unified_diff.split('\n');
                for (const line of diffLines) {
                    if (line.startsWith('+++ b/') || line.startsWith('+++ ')) {
                        const fileName = line.replace(/^\+\+\+ (b\/)?/, '');
                        const basename = fileName.split('/').pop() || fileName;
                        return basename;
                    }
                }
            }
            return null;
        },
        extractDescription: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            return t('tools.desc.showingDiff');
        }
    },
    'GeminiDiff': {
        title: t('tools.names.viewDiff'),
        icon: ICON_EDIT,
        minimal: false,  // Show full diff view
        hideDefaultError: true,
        noStatus: true,  // Always successful, stateless like Task
        input: z.object({
            unified_diff: z.string().optional().describe('Unified diff content'),
            filePath: z.string().optional().describe('File path'),
            description: z.string().optional().describe('Edit description')
        }).partial().passthrough(),
        result: z.object({
            status: z.literal('completed').describe('Always completed')
        }).partial().passthrough(),
        extractSubtitle: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            // Try to extract filename from filePath first
            if (opts.tool.input?.filePath && typeof opts.tool.input.filePath === 'string') {
                const basename = opts.tool.input.filePath.split('/').pop() || opts.tool.input.filePath;
                return basename;
            }
            // Fall back to extracting from unified diff
            if (opts.tool.input?.unified_diff && typeof opts.tool.input.unified_diff === 'string') {
                const diffLines = opts.tool.input.unified_diff.split('\n');
                for (const line of diffLines) {
                    if (line.startsWith('+++ b/') || line.startsWith('+++ ')) {
                        const fileName = line.replace(/^\+\+\+ (b\/)?/, '');
                        const basename = fileName.split('/').pop() || fileName;
                        return basename;
                    }
                }
            }
            return null;
        },
        extractDescription: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            return t('tools.desc.showingDiff');
        }
    },
    'AskUserQuestion': {
        title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            // Use first question header as title if available
            if (opts.tool.input?.questions && Array.isArray(opts.tool.input.questions) && opts.tool.input.questions.length > 0) {
                const firstQuestion = opts.tool.input.questions[0];
                if (firstQuestion.header) {
                    return firstQuestion.header;
                }
            }
            return t('tools.names.question');
        },
        icon: ICON_QUESTION,
        minimal: false,  // Always show expanded to display options
        noStatus: true,
        input: z.object({
            questions: z.array(z.object({
                question: z.string().describe('The question to ask'),
                header: z.string().describe('Short label for the question'),
                options: z.array(z.object({
                    label: z.string().describe('Option label'),
                    description: z.string().describe('Option description')
                })).describe('Available choices'),
                multiSelect: z.boolean().describe('Allow multiple selections')
            })).describe('Questions to ask the user')
        }).partial().passthrough(),
        extractSubtitle: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (opts.tool.input?.questions && Array.isArray(opts.tool.input.questions)) {
                const count = opts.tool.input.questions.length;
                if (count === 1) {
                    return opts.tool.input.questions[0].question;
                }
                return t('tools.askUserQuestion.multipleQuestions', { count });
            }
            return null;
        }
    },
    'activate_skill': {
        title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            // Extract skill name from input or result
            if (opts.tool.input?.id && typeof opts.tool.input.id === 'string') {
                return opts.tool.input.id;
            }
            if (opts.tool.result && typeof opts.tool.result === 'string') {
                const nameMatch = opts.tool.result.match(/<name>(.*?)<\/name>/);
                if (nameMatch) {
                    return nameMatch[1];
                }
            }
            return 'Activate Skill';
        },
        icon: ICON_SKILL,
        minimal: false,  // Show custom view
        noStatus: false,
        input: z.object({
            id: z.string().describe('Skill ID to activate'),
            name: z.string().optional().describe('Optional skill name'),
            include_resources: z.boolean().optional().describe('Include file tree')
        }).partial().passthrough(),
        extractSubtitle: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            // Extract description from result
            if (opts.tool.result && typeof opts.tool.result === 'string') {
                const descMatch = opts.tool.result.match(/<description>(.*?)<\/description>/);
                if (descMatch) {
                    const desc = descMatch[1];
                    return desc.length > 60 ? desc.substring(0, 60) + '...' : desc;
                }
            }
            return null;
        }
    },
    'skill_manager': {
        title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            const operation = opts.tool.input?.operation as string;
            switch (operation) {
                case 'preview_github':
                    return 'Preview Skill';
                case 'install_from_github':
                    return 'Install Skill';
                case 'get_install_status':
                    return 'Skill Status';
                case 'list_installed':
                    return 'Installed Skills';
                default:
                    return 'Skill Manager';
            }
        },
        icon: ICON_SKILL,
        minimal: false,  // Show custom view
        noStatus: false,
        input: z.object({
            operation: z.enum(['preview_github', 'install_from_github', 'get_install_status', 'list_installed']).describe('Operation to perform'),
            github_url: z.string().optional().describe('GitHub URL'),
            max_chars: z.number().optional().describe('Max chars for preview')
        }).partial().passthrough(),
        extractSubtitle: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (opts.tool.input?.github_url) {
                const url = opts.tool.input.github_url as string;
                // Extract repo/skill name from URL
                const match = url.match(/github\.com\/([^/]+\/[^/]+)/);
                if (match) {
                    return match[1];
                }
            }
            return null;
        }
    },
    'list_skills': {
        title: 'Available Skills',
        icon: ICON_SKILL,
        minimal: false,  // Show custom view
        noStatus: true,
        input: z.object({
            filter_runtime: z.enum(['browser', 'node', 'python', 'prompt', 'none']).optional().describe('Filter by runtime')
        }).partial().passthrough()
    },
    'screenshot': {
        title: 'Screenshot',
        icon: (size: number = 24, color: string = '#000') => <Ionicons name="camera-outline" size={size} color={color} />,
        minimal: false,
        noStatus: false,
        isMutable: false,
        input: z.object({}).partial().passthrough(),
    },
    'browser_navigate': {
        title: 'Navigate',
        icon: ICON_BROWSER,
        minimal: false,
        noStatus: false,
        input: z.object({
            url: z.string().describe('URL to navigate to'),
        }).partial().passthrough(),
        extractSubtitle: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            const url = opts.tool.input?.url as string | undefined;
            if (!url) return null;
            try {
                return new URL(url).hostname;
            } catch {
                return url.length > 40 ? url.slice(0, 40) + '...' : url;
            }
        },
    },
    'browser_action': {
        title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            const action = opts.tool.input?.action as string | undefined;
            if (!action) return 'Browser Action';
            const labels: Record<string, string> = {
                click: 'Click',
                type: 'Type',
                type_rich: 'Type',
                key_press: 'Key Press',
                scroll: 'Scroll',
                select: 'Select',
                upload: 'Upload',
                screenshot: 'Screenshot',
                evaluate: 'Evaluate JS',
                wait: 'Wait',
                dismiss_dialogs: 'Dismiss Dialogs',
            };
            return labels[action] || action;
        },
        icon: ICON_BROWSER,
        minimal: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            return opts.tool.input?.action !== 'screenshot';
        },
        noStatus: false,
        input: z.object({
            action: z.string().describe('Action to perform'),
            element_index: z.number().optional(),
            text: z.string().optional(),
        }).partial().passthrough(),
        extractSubtitle: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            const action = opts.tool.input?.action as string | undefined;
            if (action === 'click' && opts.tool.input?.element_index != null) {
                return `element [${opts.tool.input.element_index}]`;
            }
            if (action === 'type' || action === 'type_rich') {
                const text = opts.tool.input?.text as string | undefined;
                if (text) return text.length > 40 ? text.slice(0, 40) + '...' : text;
            }
            if (action === 'evaluate') {
                const text = opts.tool.input?.text as string | undefined;
                if (text) return text.length > 40 ? text.slice(0, 40) + '...' : text;
            }
            if (action === 'key_press') {
                return opts.tool.input?.key as string || null;
            }
            return null;
        },
    },
    'browser_state': {
        title: 'Page State',
        icon: ICON_BROWSER,
        minimal: false,
        noStatus: false,
        input: z.object({
            query: z.string().optional(),
            interactive_only: z.boolean().optional(),
        }).partial().passthrough(),
        extractSubtitle: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            const query = opts.tool.input?.query as string | undefined;
            return query ? `"${query}"` : null;
        },
    },
    'browser_manage': {
        title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            const action = opts.tool.input?.action as string | undefined;
            const labels: Record<string, string> = {
                list_tabs: 'List Tabs',
                switch_tab: 'Switch Tab',
                new_tab: 'New Tab',
                close_tab: 'Close Tab',
                close_browser: 'Close Browser',
            };
            return action ? (labels[action] || 'Browser') : 'Browser';
        },
        icon: ICON_BROWSER,
        minimal: true,
        noStatus: false,
        input: z.object({
            action: z.string().describe('Tab action'),
        }).partial().passthrough(),
    },
    'browser_screenshot': {
        title: 'Browser Screenshot',
        icon: (size: number = 24, color: string = '#000') => <Ionicons name="camera-outline" size={size} color={color} />,
        minimal: false,
        noStatus: false,
        isMutable: false,
        input: z.object({}).partial().passthrough(),
    },
    'computer_use': {
        title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            const action = opts.tool.input?.action as string | undefined;
            if (!action) return 'Computer Use';
            const labels: Record<string, string> = {
                screenshot: 'Screenshot',
                click: 'Click',
                double_click: 'Double Click',
                right_click: 'Right Click',
                type: 'Type',
                keypress: 'Key Press',
                scroll: 'Scroll',
                drag: 'Drag',
                move: 'Move',
                cursor_position: 'Cursor Position',
            };
            return labels[action] || action;
        },
        icon: (size: number = 24, color: string = '#000') => <Ionicons name="desktop-outline" size={size} color={color} />,
        minimal: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            return opts.tool.input?.action !== 'screenshot';
        },
        noStatus: false,
        input: z.object({
            action: z.string().describe('The action to perform'),
            coordinate: z.array(z.number()).optional(),
            text: z.string().optional(),
        }).partial().passthrough(),
        extractSubtitle: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            const action = opts.tool.input?.action as string | undefined;
            if (action === 'click' || action === 'double_click' || action === 'right_click') {
                const coord = opts.tool.input?.coordinate;
                if (Array.isArray(coord) && coord.length === 2) {
                    return `(${coord[0]}, ${coord[1]})`;
                }
            }
            if (action === 'type') {
                const text = opts.tool.input?.text as string | undefined;
                if (text) {
                    return text.length > 40 ? text.slice(0, 40) + '...' : text;
                }
            }
            return null;
        },
    },
    'memory': {
        title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            const action = opts.tool.input?.action as string;
            switch (action) {
                case 'save': return 'Memory Save';
                case 'recall': return 'Memory Recall';
                case 'read': return 'Memory Read';
                case 'list': return 'Memory List';
                default: return 'Memory';
            }
        },
        icon: ICON_MEMORY,
        extractSubtitle: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            const action = opts.tool.input?.action as string;
            if (action === 'recall') return opts.tool.input?.query as string || null;
            if (action === 'save' || action === 'read') return opts.tool.input?.file_path as string || null;
            return null;
        },
        extractTip: () => randomTip(TIPS.memory),
    },
    'dispatch_task': {
        title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (opts.tool.input?.tasks && Array.isArray(opts.tool.input.tasks)) {
                return `Task Graph (${opts.tool.input.tasks.length})`;
            }
            return 'Dispatch Task';
        },
        icon: ICON_DISPATCH,
        extractSubtitle: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (opts.tool.input?.task && typeof opts.tool.input.task === 'string') {
                const task = opts.tool.input.task;
                return task.length > 60 ? task.slice(0, 60) + '...' : task;
            }
            return null;
        },
        extractTip: () => randomTip(TIPS.dispatch),
    },
    'send_to_session': {
        title: 'Send to Session',
        icon: (size: number, color: string) => <Ionicons name="chatbubble-outline" size={size} color={color} />,
        minimal: true,
        extractSubtitle: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            const sid = opts.tool.input?.session_id as string;
            return sid ? `session ${sid.slice(0, 8)}...` : null;
        },
    },
    'close_task_session': {
        title: 'Close Task',
        icon: (size: number, color: string) => <Ionicons name="close-circle-outline" size={size} color={color} />,
        minimal: true,
    },
    'update_personality': {
        title: 'Update Personality',
        icon: (size: number, color: string) => <Ionicons name="sparkles-outline" size={size} color={color} />,
        minimal: true,
    },
    'schedule_task': {
        title: 'Schedule Task',
        icon: (size: number, color: string) => <Ionicons name="timer-outline" size={size} color={color} />,
        minimal: true,
        extractTip: () => randomTip(TIPS.schedule),
    },
    // Internal / remaining tools
    'websearch': {
        title: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (typeof opts.tool.input?.query === 'string') {
                return opts.tool.input.query;
            }
            return 'Web Search';
        },
        icon: ICON_WEB,
        minimal: true,
        extractSubtitle: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (typeof opts.tool.input?.query === 'string') return opts.tool.input.query;
            return null;
        },
        extractTip: () => randomTip(TIPS.web),
    },
    'image_generation': {
        title: 'Image Generation',
        icon: ICON_IMAGE,
        extractSubtitle: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            const prompt = opts.tool.input?.prompt as string;
            if (prompt) return prompt.length > 60 ? prompt.slice(0, 60) + '...' : prompt;
            return null;
        },
        extractTip: () => randomTip(TIPS.image),
    },
    'update_plan': {
        title: 'Update Plan',
        icon: ICON_PLAN,
        minimal: (opts: { metadata: Metadata | null, tool: ToolCall, messages?: Message[] }) => {
            if (typeof opts.tool.input?.explanation === 'string' && opts.tool.input.explanation.trim().length > 0) {
                return false;
            }

            if (opts.tool.input?.todos && Array.isArray(opts.tool.input.todos) && opts.tool.input.todos.length > 0) {
                return false;
            }

            return true;
        },
        input: z.object({
            explanation: z.string().optional().describe('Streaming explanation for the current plan'),
            todos: z.array(z.object({
                content: z.string().describe('The plan step content'),
                status: z.enum(['pending', 'in_progress', 'completed']).describe('The status of the plan step'),
                priority: z.enum(['high', 'medium', 'low']).optional().describe('The priority of the plan step'),
                id: z.string().optional().describe('Unique identifier for the plan step')
            }).passthrough()).optional().describe('The current plan steps')
        }).partial().passthrough(),
        extractDescription: (opts: { metadata: Metadata | null, tool: ToolCall }) => {
            if (typeof opts.tool.input?.explanation === 'string' && opts.tool.input.explanation.trim().length > 0) {
                return opts.tool.input.explanation.trim();
            }
            if (Array.isArray(opts.tool.input?.todos) && opts.tool.input.todos.length > 0) {
                return t('tools.desc.todoListCount', { count: opts.tool.input.todos.length });
            }
            return 'Update Plan';
        },
        extractTip: () => randomTip(TIPS.plan),
    },
    'ask_persona': {
        title: 'Ask Persona',
        icon: ICON_QUESTION,
        minimal: true,
    },
    'get_session_output': {
        title: 'Get Session Output',
        icon: (size: number, color: string) => <Octicons name="terminal" size={size} color={color} />,
        minimal: true,
    },
    'list_task_sessions': {
        title: 'List Tasks',
        icon: (size: number, color: string) => <Octicons name="tasklist" size={size} color={color} />,
        minimal: true,
    },
    'list_scheduled_tasks': {
        title: 'List Scheduled Tasks',
        icon: (size: number, color: string) => <Ionicons name="timer-outline" size={size} color={color} />,
        minimal: true,
    },
    'delete_scheduled_task': {
        title: 'Delete Scheduled Task',
        icon: (size: number, color: string) => <Ionicons name="trash-outline" size={size} color={color} />,
        minimal: true,
    },
    'skill_context': {
        title: 'Skill Context',
        icon: ICON_SKILL,
        minimal: true,
        extractTip: () => randomTip(TIPS.skill),
    },
    // Internal Claude Code tool for loading deferred tools — no user-visible output
    'ToolSearch': {
        icon: ICON_SEARCH,
        hidden: true,
    },
    // Codex Guardian auto-approval review — emitted by Codex adapter as a PermissionRequest
    'CodexGuardian': {
        title: 'Codex Guardian',
        icon: (size: number, color: string) => <Ionicons name="shield-checkmark-outline" size={size} color={color} />,
        minimal: false,
        noStatus: true,
    },
} satisfies Record<string, {
    title?: string | ((opts: { metadata: Metadata | null, tool: ToolCall }) => string);
    icon: (size: number, color: string) => React.ReactNode;
    noStatus?: boolean;
    hideDefaultError?: boolean;
    hidden?: boolean;
    isMutable?: boolean;
    input?: z.ZodObject<any>;
    result?: z.ZodObject<any>;
    minimal?: boolean | ((opts: { metadata: Metadata | null, tool: ToolCall, messages?: Message[] }) => boolean);
    extractDescription?: (opts: { metadata: Metadata | null, tool: ToolCall }) => string;
    extractSubtitle?: (opts: { metadata: Metadata | null, tool: ToolCall }) => string | null;
    extractStatus?: (opts: { metadata: Metadata | null, tool: ToolCall }) => string | null;
    extractTip?: (opts: { metadata: Metadata | null, tool: ToolCall }) => string | null;
}>;

/**
 * Check if a tool is mutable (can potentially modify files)
 * @param toolName The name of the tool to check
 * @returns true if the tool is mutable or unknown, false if it's read-only
 */
export function isMutableTool(toolName: string): boolean {
    const tool = knownTools[toolName as keyof typeof knownTools];
    if (tool) {
        if ('isMutable' in tool) {
            return tool.isMutable === true;
        } else {
            return false;
        }
    }
    // If tool is unknown, assume it's mutable to be safe
    return true;
}
