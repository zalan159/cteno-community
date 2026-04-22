/**
 * A2UI Renderer — resolves flat component arrays into a tree and renders
 * each component using the registry. Aligned with Google A2UI v0.9.
 */
import React from 'react';
import { View } from 'react-native';
import { useUnistyles } from 'react-native-unistyles';
import { Text } from '@/components/StyledText';
import type { A2uiComponent, A2uiSurface, A2uiAction, A2uiActionEvent } from './types';
import { COMPONENT_REGISTRY, PARENT_COMPONENTS } from './registry';

interface A2uiRendererProps {
  surfaces: A2uiSurface[];
  onAction: (event: A2uiActionEvent) => void;
}

export function A2uiRenderer({ surfaces, onAction }: A2uiRendererProps) {
  const { theme } = useUnistyles();

  if (!surfaces || surfaces.length === 0) {
    return (
      <View style={{ flex: 1, justifyContent: 'center', alignItems: 'center' }}>
        <Text style={{ fontSize: 14, color: theme.colors.textSecondary }}>
          AIUI 已就绪
        </Text>
      </View>
    );
  }

  return (
    <View style={{ flex: 1 }}>
      {surfaces.map((surface) => (
        <SurfaceRenderer
          key={surface.surfaceId}
          surface={surface}
          onAction={onAction}
        />
      ))}
    </View>
  );
}

/** Renders a single surface by resolving its flat component list into a tree. */
function SurfaceRenderer({
  surface,
  onAction,
}: {
  surface: A2uiSurface;
  onAction: (event: A2uiActionEvent) => void;
}) {
  const { components, surfaceId } = surface;

  // Build id → component lookup map
  const componentMap = React.useMemo(() => {
    const map = new Map<string, A2uiComponent>();
    for (const comp of components) {
      if (comp.id) map.set(comp.id, comp);
    }
    return map;
  }, [components]);

  // Find which IDs are referenced as children (non-root)
  const childIds = React.useMemo(() => {
    const ids = new Set<string>();
    for (const comp of components) {
      if (comp.children && Array.isArray(comp.children)) {
        for (const childId of comp.children) {
          ids.add(childId);
        }
      }
    }
    return ids;
  }, [components]);

  // Root components = those not referenced as children
  const rootComponents = React.useMemo(
    () => components.filter((c) => !childIds.has(c.id)),
    [components, childIds],
  );

  const handleAction = React.useCallback(
    (componentId: string, action: A2uiAction) => {
      onAction({
        surfaceId,
        componentId,
        event: action.event,
      });
    },
    [surfaceId, onAction],
  );

  return (
    <View style={{ flex: 1 }}>
      {rootComponents.map((comp) => (
        <ComponentNode
          key={comp.id}
          comp={comp}
          componentMap={componentMap}
          onAction={handleAction}
        />
      ))}
    </View>
  );
}

/** Recursively renders a component and its children. */
function ComponentNode({
  comp,
  componentMap,
  onAction,
}: {
  comp: A2uiComponent;
  componentMap: Map<string, A2uiComponent>;
  onAction: (componentId: string, action: A2uiAction) => void;
}) {
  const { theme } = useUnistyles();
  const Component = COMPONENT_REGISTRY[comp.component];

  if (!Component) {
    return (
      <View style={{ padding: 4 }}>
        <Text style={{ fontSize: 11, color: theme.colors.textSecondary }}>
          [Unknown: {comp.component}]
        </Text>
      </View>
    );
  }

  // Extract component-specific props (remove id, component, children)
  const { id, component, children, action, ...props } = comp;

  // If this component can have children, pass a renderChildren function
  const isParent = PARENT_COMPONENTS.has(component);
  if (isParent && children && Array.isArray(children) && children.length > 0) {
    props.renderChildren = () =>
      children.map((childId: string) => {
        const childComp = componentMap.get(childId);
        if (!childComp) return null;
        return (
          <ComponentNode
            key={childId}
            comp={childComp}
            componentMap={componentMap}
            onAction={onAction}
          />
        );
      });
  }

  // Pass action handling
  if (action) {
    props.action = action;
    props.onAction = (a: A2uiAction) => onAction(id, a);
  }

  return <Component {...props} />;
}
