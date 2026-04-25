import * as React from "react";
import { FlatList, Image, Pressable, StyleSheet, View } from "react-native";
import type { Message, ToolCallMessage } from "@cteno/client-sync";
import { ImagePreviewModal, MarkdownView, Surface, Text, uiColors } from "@cteno/client-ui";
import { ToolCallCard } from "./ToolCallCard";

export function MessageList(props: {
  messages: Message[];
  onOptionPress?: (text: string) => void;
  onOpenToolDetail?: (message: ToolCallMessage) => void;
  onSendToolToBackground?: (message: ToolCallMessage) => Promise<void> | void;
}) {
  const [previewUri, setPreviewUri] = React.useState<string | null>(null);
  return (
    <>
      <FlatList
        data={props.messages}
        keyExtractor={(item) => item.id}
        contentContainerStyle={styles.list}
        renderItem={({ item }) => (
          <MessageRow
            message={item}
            onPreview={setPreviewUri}
            onOpenToolDetail={props.onOpenToolDetail}
            onSendToolToBackground={props.onSendToolToBackground}
          />
        )}
      />
      <ImagePreviewModal uri={previewUri} visible={!!previewUri} onClose={() => setPreviewUri(null)} />
    </>
  );
}

function MessageRow(props: {
  message: Message;
  onPreview: (uri: string) => void;
  onOpenToolDetail?: (message: ToolCallMessage) => void;
  onSendToolToBackground?: (message: ToolCallMessage) => Promise<void> | void;
}) {
  switch (props.message.kind) {
    case "user-text":
      return (
        <View style={styles.userRow}>
          <Surface style={styles.userBubble}>
            {props.message.images?.map((image, index) => {
              const uri = image.data ? `data:${image.media_type};base64,${image.data}` : image.file_path ?? null;
              if (!uri) return null;
              return (
                <Pressable key={index} onPress={() => props.onPreview(uri)}>
                  <Image source={{ uri }} style={styles.attachment} />
                </Pressable>
              );
            })}
            <MarkdownView markdown={props.message.displayText ?? props.message.text} />
          </Surface>
        </View>
      );
    case "agent-text":
      return (
        <View style={styles.agentRow}>
          <MarkdownView markdown={props.message.text} />
        </View>
      );
    case "tool-call":
      return (
        <ToolCallCard
          tool={props.message.tool}
          onOpenDetail={props.onOpenToolDetail ? () => props.onOpenToolDetail?.(props.message as ToolCallMessage) : undefined}
          onSendToBackground={
            props.onSendToolToBackground ? () => props.onSendToolToBackground?.(props.message as ToolCallMessage) : undefined
          }
        />
      );
    case "agent-event":
      return (
        <Surface style={styles.event}>
          <Text style={styles.eventTitle}>{props.message.event.title ?? props.message.event.type}</Text>
          {props.message.event.message ? <Text style={styles.eventText}>{props.message.event.message}</Text> : null}
        </Surface>
      );
    default:
      return null;
  }
}

const styles = StyleSheet.create({
  list: {
    gap: 12,
    padding: 12,
  },
  userRow: {
    alignItems: "flex-end",
  },
  userBubble: {
    backgroundColor: "#EAF3FF",
    maxWidth: "82%",
    padding: 12,
  },
  agentRow: {
    maxWidth: 720,
  },
  attachment: {
    borderRadius: 6,
    height: 120,
    marginBottom: 8,
    width: 120,
  },
  event: {
    backgroundColor: "#F7F9FB",
    padding: 10,
  },
  eventTitle: {
    fontFamily: "IBMPlexSans-SemiBold",
  },
  eventText: {
    color: uiColors.muted,
  },
});
