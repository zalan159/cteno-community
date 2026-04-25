import * as React from "react";
import { ActivityIndicator, StyleSheet, View, type ViewStyle } from "react-native";
import { Text, uiColors } from "./primitives";

export function Deferred(props: { children: React.ReactNode; fallback?: React.ReactNode; ready?: boolean }) {
  if (props.ready === false) return props.fallback ?? <ActivityIndicator />;
  return <>{props.children}</>;
}

export function ShimmerView(props: { style?: ViewStyle | ViewStyle[] }) {
  return <View style={[styles.shimmer, props.style]} />;
}

export function EmptyState(props: { title: string; detail?: string; action?: React.ReactNode }) {
  return (
    <View style={styles.empty}>
      <Text style={styles.emptyTitle}>{props.title}</Text>
      {props.detail ? <Text style={styles.emptyDetail}>{props.detail}</Text> : null}
      {props.action}
    </View>
  );
}

export function FloatingOverlay(props: { children: React.ReactNode; style?: ViewStyle }) {
  return <View style={[styles.overlay, props.style]}>{props.children}</View>;
}

const styles = StyleSheet.create({
  shimmer: {
    backgroundColor: "#E9EEF2",
    borderRadius: 6,
    minHeight: 16,
    overflow: "hidden",
  },
  empty: {
    alignItems: "center",
    gap: 8,
    justifyContent: "center",
    padding: 24,
  },
  emptyTitle: {
    fontFamily: "IBMPlexSans-SemiBold",
    fontSize: 16,
  },
  emptyDetail: {
    color: uiColors.muted,
    maxWidth: 360,
    textAlign: "center",
  },
  overlay: {
    backgroundColor: "rgba(255,255,255,0.94)",
    borderColor: uiColors.line,
    borderRadius: 8,
    borderWidth: StyleSheet.hairlineWidth,
    shadowColor: "#000",
    shadowOffset: { width: 0, height: 12 },
    shadowOpacity: 0.16,
    shadowRadius: 24,
  },
});
