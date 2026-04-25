import type { VendorName } from "./session";

export interface ModelOptionDisplay {
  id: string;
  label: string;
  vendor: VendorName;
  description?: string | null;
  contextWindow?: number | null;
  supportsReasoningEffort?: boolean;
}

export type ReasoningEffort = "low" | "medium" | "high" | "xhigh";

export interface AIBackendProfile {
  id: string;
  name: string;
  vendor: VendorName;
  model: string;
  effort?: ReasoningEffort;
}
