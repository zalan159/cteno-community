import * as React from 'react';
import { EditView } from './EditView';
import { BashView } from './BashView';
import { Message, ToolCall } from '@/sync/typesMessage';
import { Metadata } from '@/sync/storageTypes';
import { WriteView } from './WriteView';
import { TodoView } from './TodoView';
import { ExitPlanToolView } from './ExitPlanToolView';
import { MultiEditView } from './MultiEditView';
import { TaskView } from './TaskView';
import { BashViewFull } from './BashViewFull';
import { EditViewFull } from './EditViewFull';
import { MultiEditViewFull } from './MultiEditViewFull';
import { CodexBashView } from './CodexBashView';
import { CodexPatchView } from './CodexPatchView';
import { CodexDiffView } from './CodexDiffView';
import { AskUserQuestionView } from './AskUserQuestionView';
import { ElicitationView } from './ElicitationView';
import { GeminiEditView } from './GeminiEditView';
import { GeminiExecuteView } from './GeminiExecuteView';
import { ActivateSkillView } from './ActivateSkillView';
import { SkillManagerView } from './SkillManagerView';
import { ListSkillsView } from './ListSkillsView';
import { FetchView } from './FetchView';
import { ComputerUseView } from './ComputerUseView';
import { ScreenshotView } from './ScreenshotView';
import { MemoryView } from './MemoryView';
import { DispatchTaskView } from './DispatchTaskView';
import { BrowserNavigateView } from './BrowserNavigateView';
import { BrowserActionView } from './BrowserActionView';
import { BrowserStateView } from './BrowserStateView';
import { BrowserManageView } from './BrowserManageView';

export type ToolViewProps = {
    tool: ToolCall;
    metadata: Metadata | null;
    messages: Message[];
    sessionId?: string;
}

// Type for tool view components
export type ToolViewComponent = React.ComponentType<ToolViewProps>;

// Registry of tool-specific view components
export const toolViewRegistry: Record<string, ToolViewComponent> = {
    Edit: EditView,
    Bash: BashView,
    CodexBash: CodexBashView,
    CodexPatch: CodexPatchView,
    CodexDiff: CodexDiffView,
    Write: WriteView,
    TodoWrite: TodoView,
    update_plan: TodoView,
    ExitPlanMode: ExitPlanToolView,
    exit_plan_mode: ExitPlanToolView,
    MultiEdit: MultiEditView,
    Task: TaskView,
    Agent: TaskView,
    AskUserQuestion: AskUserQuestionView,
    Elicitation: ElicitationView,
    activate_skill: ActivateSkillView,
    skill_manager: SkillManagerView,
    list_skills: ListSkillsView,
    fetch: FetchView,
    computer_use: ComputerUseView,
    screenshot: ScreenshotView,
    browser_screenshot: ScreenshotView,
    browser_navigate: BrowserNavigateView,
    browser_action: BrowserActionView,
    browser_state: BrowserStateView,
    browser_manage: BrowserManageView,
    // Gemini tools (lowercase)
    edit: GeminiEditView,
    execute: GeminiExecuteView,
    // Cteno tools
    memory: MemoryView,
    dispatch_task: DispatchTaskView,
};

export const toolFullViewRegistry: Record<string, ToolViewComponent> = {
    Bash: BashViewFull,
    CodexBash: CodexBashView,
    Edit: EditViewFull,
    MultiEdit: MultiEditViewFull,
    Task: TaskView,
    Agent: TaskView,
};

// Helper function to get the appropriate view component for a tool
export function getToolViewComponent(toolName: string): ToolViewComponent | null {
    return toolViewRegistry[toolName] || null;
}

// Helper function to get the full view component for a tool
export function getToolFullViewComponent(toolName: string): ToolViewComponent | null {
    return toolFullViewRegistry[toolName] || null;
}

// Export individual components
export { EditView } from './EditView';
export { BashView } from './BashView';
export { CodexBashView } from './CodexBashView';
export { CodexPatchView } from './CodexPatchView';
export { CodexDiffView } from './CodexDiffView';
export { BashViewFull } from './BashViewFull';
export { EditViewFull } from './EditViewFull';
export { MultiEditViewFull } from './MultiEditViewFull';
export { ExitPlanToolView } from './ExitPlanToolView';
export { MultiEditView } from './MultiEditView';
export { TaskView } from './TaskView';
export { AskUserQuestionView } from './AskUserQuestionView';
export { ElicitationView } from './ElicitationView';
export { GeminiEditView } from './GeminiEditView';
export { GeminiExecuteView } from './GeminiExecuteView';
export { ActivateSkillView } from './ActivateSkillView';
export { SkillManagerView } from './SkillManagerView';
export { ListSkillsView } from './ListSkillsView';
export { FetchView } from './FetchView';
export { ComputerUseView } from './ComputerUseView';
export { ScreenshotView } from './ScreenshotView';
export { MemoryView } from './MemoryView';
export { DispatchTaskView } from './DispatchTaskView';
export { BrowserNavigateView } from './BrowserNavigateView';
export { BrowserActionView } from './BrowserActionView';
export { BrowserStateView } from './BrowserStateView';
export { BrowserManageView } from './BrowserManageView';
