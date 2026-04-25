import * as React from "react";
import { StyleSheet, View } from "react-native";
import { Button, TextField } from "@cteno/client-ui";

export function AgentComposer(props: {
  placeholder?: string;
  disabled?: boolean;
  onSubmit: (text: string) => Promise<void> | void;
}) {
  const [text, setText] = React.useState("");
  const [sending, setSending] = React.useState(false);
  const canSend = !!text.trim() && !props.disabled && !sending;
  const submit = React.useCallback(async () => {
    if (!canSend) return;
    const value = text.trim();
    setSending(true);
    try {
      await props.onSubmit(value);
      setText("");
    } finally {
      setSending(false);
    }
  }, [canSend, props, text]);

  return (
    <View style={styles.wrap}>
      <TextField
        value={text}
        onChangeText={setText}
        placeholder={props.placeholder ?? "Message agent"}
        multiline
        style={styles.input}
      />
      <Button title="Send" disabled={!canSend} loading={sending} onPress={submit} />
    </View>
  );
}

const styles = StyleSheet.create({
  wrap: {
    alignItems: "flex-end",
    flexDirection: "row",
    gap: 8,
    padding: 10,
  },
  input: {
    flex: 1,
    maxHeight: 120,
    minHeight: 42,
    paddingVertical: 9,
  },
});
