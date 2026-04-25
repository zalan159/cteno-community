export interface MessageMeta {
  vendor?: string;
  flavor?: string;
  model?: string;
  [key: string]: unknown;
}

export interface AgentEvent {
  type: string;
  title?: string;
  message?: string;
  data?: unknown;
}

export type ToolCallState = "running" | "completed" | "error";

export interface ToolPermission {
  id: string;
  status: "pending" | "approved" | "denied" | "canceled";
  reason?: string;
  mode?: string;
  allowedTools?: string[];
  decision?: "approved" | "approved_for_session" | "denied" | "abort";
  date?: number;
}

export interface ToolCall {
  name: string;
  state: ToolCallState;
  input: unknown;
  createdAt: number;
  startedAt: number | null;
  completedAt: number | null;
  description: string | null;
  result?: unknown;
  callId?: string;
  permission?: ToolPermission;
}

export interface ImageAttachment {
  media_type: string;
  data?: string;
  file_id?: string;
  file_path?: string;
}

export interface BaseMessage {
  id: string;
  localId: string | null;
  createdAt: number;
  meta?: MessageMeta;
}

export interface UserTextMessage extends BaseMessage {
  kind: "user-text";
  text: string;
  displayText?: string;
  images?: ImageAttachment[];
}

export interface AgentTextMessage extends BaseMessage {
  kind: "agent-text";
  text: string;
  isThinking?: boolean;
  images?: ImageAttachment[];
}

export interface ToolCallMessage extends BaseMessage {
  kind: "tool-call";
  tool: ToolCall;
  children: Message[];
}

export interface AgentEventMessage extends BaseMessage {
  kind: "agent-event";
  event: AgentEvent;
}

export type Message = UserTextMessage | AgentTextMessage | ToolCallMessage | AgentEventMessage;
