export interface A2uiComponent {
  id: string;
  component: string;
  children?: string[];
  action?: A2uiAction;
  [key: string]: unknown;
}

export interface A2uiAction {
  event: {
    name: string;
    data?: Record<string, unknown>;
  };
}

export interface A2uiSurface {
  surfaceId: string;
  catalogId: string;
  components: A2uiComponent[];
  dataModel: unknown;
  version: number;
}

export type A2uiState = A2uiSurface[];

export interface A2uiActionEvent {
  surfaceId: string;
  componentId: string;
  event: {
    name: string;
    data?: Record<string, unknown>;
  };
}
