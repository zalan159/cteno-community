import * as React from "react";
import { StyleSheet, View } from "react-native";
import { Text } from "./primitives";

export function calculateDiff(before: string, after: string) {
  const a = before.split("\n");
  const b = after.split("\n");
  const rows: Array<{ type: "same" | "add" | "remove"; text: string }> = [];
  const max = Math.max(a.length, b.length);
  for (let i = 0; i < max; i += 1) {
    if (a[i] === b[i]) rows.push({ type: "same", text: a[i] ?? "" });
    else {
      if (a[i] !== undefined) rows.push({ type: "remove", text: a[i] });
      if (b[i] !== undefined) rows.push({ type: "add", text: b[i] });
    }
  }
  return rows;
}

export function DiffView(props: { before: string; after: string }) {
  return (
    <View style={styles.wrap}>
      {calculateDiff(props.before, props.after).map((row, index) => (
        <Text key={index} style={[styles.row, styles[row.type]]}>
          {row.type === "add" ? "+ " : row.type === "remove" ? "- " : "  "}
          {row.text}
        </Text>
      ))}
    </View>
  );
}

const styles = StyleSheet.create({
  wrap: {
    backgroundColor: "#101418",
    borderRadius: 6,
    overflow: "hidden",
    paddingVertical: 6,
  },
  row: {
    color: "#D9E2EC",
    fontFamily: "IBMPlexMono-Regular",
    fontSize: 12,
    lineHeight: 18,
    paddingHorizontal: 10,
  },
  same: {},
  add: {
    backgroundColor: "rgba(36,138,61,0.25)",
  },
  remove: {
    backgroundColor: "rgba(217,45,32,0.22)",
  },
});
