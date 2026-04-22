/**
 * A2UI Component Registry — maps component type strings to React components.
 * This is the "catalog" that defines what the client can render.
 */
import type { ComponentType } from 'react';

import { A2uiContainer } from './components/A2uiContainer';
import { A2uiRow } from './components/A2uiRow';
import { A2uiColumn } from './components/A2uiColumn';
import { A2uiCard } from './components/A2uiCard';
import { A2uiDivider } from './components/A2uiDivider';
import { A2uiText } from './components/A2uiText';
import { A2uiProgress } from './components/A2uiProgress';
import { A2uiMetricCard } from './components/A2uiMetricCard';
import { A2uiMetricsGrid } from './components/A2uiMetricsGrid';
import { A2uiStatusIndicator } from './components/A2uiStatusIndicator';
import { A2uiBadge } from './components/A2uiBadge';
import { A2uiList } from './components/A2uiList';
import { A2uiListItem } from './components/A2uiListItem';
import { A2uiChecklistItem } from './components/A2uiChecklistItem';
import { A2uiButton } from './components/A2uiButton';
import { A2uiButtonGroup } from './components/A2uiButtonGroup';
import { A2uiImage } from './components/A2uiImage';
import { A2uiIcon } from './components/A2uiIcon';
import { A2uiActivityFeed } from './components/A2uiActivityFeed';

/** Components that can have children (layout/container types) */
export const PARENT_COMPONENTS = new Set([
  'Container', 'Row', 'Column', 'Card', 'List', 'ButtonGroup',
]);

export const COMPONENT_REGISTRY: Record<string, ComponentType<any>> = {
  // Layout
  Container: A2uiContainer,
  Row: A2uiRow,
  Column: A2uiColumn,
  Card: A2uiCard,
  Divider: A2uiDivider,
  // Data Display
  Text: A2uiText,
  Progress: A2uiProgress,
  MetricCard: A2uiMetricCard,
  MetricsGrid: A2uiMetricsGrid,
  StatusIndicator: A2uiStatusIndicator,
  Badge: A2uiBadge,
  // Lists
  List: A2uiList,
  ListItem: A2uiListItem,
  ChecklistItem: A2uiChecklistItem,
  // Interactive
  Button: A2uiButton,
  ButtonGroup: A2uiButtonGroup,
  // Media
  Image: A2uiImage,
  Icon: A2uiIcon,
  // Composite
  ActivityFeed: A2uiActivityFeed,
};
