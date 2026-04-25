import type { MessageMeta } from "./messages";

export type VendorName = "cteno" | "claude" | "codex" | "gemini" | string;

export interface Session {
  id: string;
  title?: string | null;
  machineId?: string | null;
  workspacePath?: string | null;
  vendor?: VendorName;
  createdAt: number;
  updatedAt: number;
  metadata?: MessageMeta | null;
}

export interface Machine {
  id: string;
  name: string;
  platform?: string;
  online?: boolean;
}

export interface SessionListViewItem {
  session: Session;
  latestText?: string | null;
  unread?: boolean;
}
