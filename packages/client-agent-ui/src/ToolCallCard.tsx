import * as React from "react";
import { StyleSheet, View } from "react-native";
import { Ionicons } from "@expo/vector-icons";
import type { ToolCall } from "@cteno/client-sync";
import { Button, MarkdownView, StatusDot, Surface, Text, uiColors } from "@cteno/client-ui";

export function ToolCallCard(props: {
  tool: ToolCall;
  onOpenDetail?: () => void;
  onSendToBackground?: () => Promise<void> | void;
}) {
  const [busy, setBusy] = React.useState(false);
  const statusTone = props.tool.state === "completed" ? "success" : props.tool.state === "error" ? "danger" : "warning";
  const handleBackground = React.useCallback(async () => {
    if (!props.onSendToBackground) return;
    setBusy(true);
    try {
      await props.onSendToBackground();
    } finally {
      setBusy(false);
    }
  }, [props.onSendToBackground]);

  return (
    <Surface style={styles.card}>
      <View style={styles.header}>
        <Ionicons name="construct-outline" size={18} color={uiColors.coal} />
        <View style={{ flex: 1 }}>
          <Text style={styles.title}>{props.tool.name || "tool"}</Text>
          {props.tool.description ? <Text style={styles.description}>{props.tool.description}</Text> : null}
        </View>
        <StatusDot tone={statusTone} label={props.tool.state} />
      </View>
      {props.tool.result ? (
        <View style={styles.result}>
          <MarkdownView markdown={stringifyResult(props.tool.result)} />
        </View>
      ) : null}
      {(props.onOpenDetail || props.onSendToBackground) ? (
        <View style={styles.actions}>
          {props.onOpenDetail ? <Button title="Open" variant="ghost" onPress={props.onOpenDetail} /> : null}
          {props.onSendToBackground ? (
            <Button title="Background" variant="ghost" loading={busy} onPress={handleBackground} />
          ) : null}
        </View>
      ) : null}
    </Surface>
  );
}

function stringifyResult(result: unknown) {
  if (typeof result === "string") return result;
  try {
    return "```json\n" + JSON.stringify(result, null, 2) + "\n```";
  } catch {
    return String(result);
  }
}

const styles = StyleSheet.create({
  card: {
    gap: 10,
    padding: 12,
  },
  header: {
    alignItems: "center",
    flexDirection: "row",
    gap: 10,
  },
  title: {
    fontFamily: "IBMPlexSans-SemiBold",
  },
  description: {
    color: uiColors.muted,
    fontSize: 12,
  },
  result: {
    borderTopColor: uiColors.line,
    borderTopWidth: StyleSheet.hairlineWidth,
    paddingTop: 10,
  },
  actions: {
    flexDirection: "row",
    gap: 8,
    justifyContent: "flex-end",
  },
});
