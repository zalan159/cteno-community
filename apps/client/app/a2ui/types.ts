/**
 * A2UI Protocol Types — aligned with Google A2UI v0.9
 *
 * Declarative UI components rendered natively by React Native.
 */

/** A single UI component in the flat component list. */
export interface A2uiComponent {
  /** Unique component ID */
  id: string;
  /** Component type from catalog (e.g. "Text", "Progress", "Card") */
  component: string;
  /** Child component IDs for parent-child relationships */
  children?: string[];
  /** Action handler for interactive components */
  action?: A2uiAction;
  /** All other component-specific props */
  [key: string]: any;
}

/** Action definition for interactive components. */
export interface A2uiAction {
  event: {
    name: string;
    data?: Record<string, any>;
  };
}

/** A rendering surface containing components and data. */
export interface A2uiSurface {
  surfaceId: string;
  catalogId: string;
  components: A2uiComponent[];
  dataModel: any;
  version: number;
}

/** State of all surfaces for an agent. */
export type A2uiState = A2uiSurface[];

/** Callback for user interactions with A2UI components. */
export interface A2uiActionEvent {
  surfaceId: string;
  componentId: string;
  event: {
    name: string;
    data?: Record<string, any>;
  };
}
