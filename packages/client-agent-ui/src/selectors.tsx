import * as React from "react";
import { Pressable, StyleSheet, View } from "react-native";
import type { ModelOptionDisplay, ReasoningEffort } from "@cteno/client-sync";
import { Text, uiColors } from "@cteno/client-ui";

export function ModelSelector(props: {
  models: ModelOptionDisplay[];
  value: string | null;
  onChange: (modelId: string) => void;
}) {
  return (
    <View style={styles.wrap}>
      {props.models.map((model) => (
        <Pressable
          key={model.id}
          onPress={() => props.onChange(model.id)}
          style={[styles.segment, props.value === model.id && styles.selected]}
        >
          <Text style={[styles.label, props.value === model.id && styles.selectedLabel]}>{model.label}</Text>
        </Pressable>
      ))}
    </View>
  );
}

export function EffortSelector(props: { value: ReasoningEffort; onChange: (effort: ReasoningEffort) => void }) {
  return (
    <View style={styles.wrap}>
      {(["low", "medium", "high", "xhigh"] as const).map((effort) => (
        <Pressable
          key={effort}
          onPress={() => props.onChange(effort)}
          style={[styles.segment, props.value === effort && styles.selected]}
        >
          <Text style={[styles.label, props.value === effort && styles.selectedLabel]}>{effort}</Text>
        </Pressable>
      ))}
    </View>
  );
}

export function PermissionModeSelector(props: {
  value: string;
  options: string[];
  onChange: (mode: string) => void;
}) {
  return (
    <View style={styles.wrap}>
      {props.options.map((option) => (
        <Pressable
          key={option}
          onPress={() => props.onChange(option)}
          style={[styles.segment, props.value === option && styles.selected]}
        >
          <Text style={[styles.label, props.value === option && styles.selectedLabel]}>{option}</Text>
        </Pressable>
      ))}
    </View>
  );
}

const styles = StyleSheet.create({
  wrap: {
    backgroundColor: "#E8EEF3",
    borderRadius: 7,
    flexDirection: "row",
    gap: 2,
    padding: 2,
  },
  segment: {
    borderRadius: 5,
    paddingHorizontal: 10,
    paddingVertical: 6,
  },
  selected: {
    backgroundColor: "#FFFFFF",
  },
  label: {
    color: uiColors.muted,
    fontSize: 12,
  },
  selectedLabel: {
    color: uiColors.ink,
    fontFamily: "IBMPlexSans-SemiBold",
  },
});
