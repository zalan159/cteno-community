import * as React from "react";
import { StyleSheet, View } from "react-native";
import { Button, ListItem, StatusDot, Surface, Text, uiColors } from "@cteno/client-ui";
import type { A2uiActionEvent, A2uiComponent, A2uiSurface } from "./types";

export function A2uiRenderer(props: {
  surface: A2uiSurface;
  onAction?: (event: A2uiActionEvent) => void;
}) {
  const components = React.useMemo(
    () => Object.fromEntries(props.surface.components.map((component) => [component.id, component])),
    [props.surface.components],
  );
  const roots = props.surface.components.filter(
    (component) => !props.surface.components.some((candidate) => candidate.children?.includes(component.id)),
  );
  return (
    <View style={styles.wrap}>
      {roots.map((component) => renderComponent(component, components, props.surface.surfaceId, props.onAction))}
    </View>
  );
}

function renderComponent(
  component: A2uiComponent,
  components: Record<string, A2uiComponent>,
  surfaceId: string,
  onAction?: (event: A2uiActionEvent) => void,
) {
  const children = component.children?.map((id) => components[id]).filter(Boolean) ?? [];
  const renderedChildren = children.map((child) => renderComponent(child, components, surfaceId, onAction));
  const action = component.action
    ? () => onAction?.({ surfaceId, componentId: component.id, event: component.action!.event })
    : undefined;
  switch (component.component) {
    case "Text":
      return (
        <Text key={component.id} style={component.variant === "title" ? styles.title : undefined}>
          {String(component.text ?? "")}
        </Text>
      );
    case "Button":
      return <Button key={component.id} title={String(component.label ?? component.text ?? "Action")} onPress={action} />;
    case "Card":
      return (
        <Surface key={component.id} style={styles.card}>
          {renderedChildren}
        </Surface>
      );
    case "ListItem":
      return (
        <ListItem
          key={component.id}
          title={String(component.title ?? "")}
          subtitle={component.subtitle ? String(component.subtitle) : undefined}
          onPress={action}
        />
      );
    case "Status":
      return <StatusDot key={component.id} tone={component.tone as any} label={String(component.label ?? "")} />;
    default:
      return (
        <Surface key={component.id} style={styles.unknown}>
          <Text style={styles.unknownText}>Unsupported A2UI component: {component.component}</Text>
          {renderedChildren}
        </Surface>
      );
  }
}

const styles = StyleSheet.create({
  wrap: {
    gap: 10,
  },
  title: {
    fontFamily: "IBMPlexSans-SemiBold",
    fontSize: 18,
  },
  card: {
    gap: 8,
    padding: 12,
  },
  unknown: {
    borderColor: uiColors.warning,
    padding: 10,
  },
  unknownText: {
    color: uiColors.warning,
  },
});
