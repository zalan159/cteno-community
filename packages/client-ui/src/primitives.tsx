import * as React from "react";
import {
  ActivityIndicator,
  Image,
  Modal,
  Pressable,
  ScrollView,
  StyleSheet,
  Text as RNText,
  TextInput,
  type TextInputProps,
  type TextProps,
  type TextStyle,
  View,
  type ViewStyle,
} from "react-native";
import { Ionicons } from "@expo/vector-icons";

export type Tone = "neutral" | "accent" | "success" | "warning" | "danger";

export const uiColors = {
  ink: "#15171A",
  muted: "#687076",
  faint: "#8C959E",
  line: "#DCE2E7",
  panel: "#FFFFFF",
  canvas: "#F5F7F8",
  accent: "#0A84FF",
  success: "#248A3D",
  warning: "#B76E00",
  danger: "#D92D20",
  coal: "#20252B",
};

export function Text({ style, ...props }: TextProps) {
  return <RNText {...props} style={[styles.text, style]} />;
}

export function Surface(props: { children: React.ReactNode; style?: ViewStyle | ViewStyle[] }) {
  return <View style={[styles.surface, props.style]}>{props.children}</View>;
}

export function SectionHeader(props: { title: string; detail?: string; action?: React.ReactNode }) {
  return (
    <View style={styles.sectionHeader}>
      <View style={{ flex: 1 }}>
        <Text style={styles.sectionTitle}>{props.title}</Text>
        {props.detail ? <Text style={styles.sectionDetail}>{props.detail}</Text> : null}
      </View>
      {props.action}
    </View>
  );
}

export function IconButton(props: {
  icon: keyof typeof Ionicons.glyphMap;
  label: string;
  onPress?: () => void;
  disabled?: boolean;
  tone?: Tone;
}) {
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={props.label}
      disabled={props.disabled}
      onPress={props.onPress}
      style={({ pressed }) => [
        styles.iconButton,
        props.disabled && styles.disabled,
        pressed && styles.pressed,
      ]}
    >
      <Ionicons name={props.icon} size={18} color={toneColor(props.tone)} />
    </Pressable>
  );
}

export function Button(props: {
  title: string;
  onPress?: () => void;
  disabled?: boolean;
  loading?: boolean;
  tone?: Tone;
  variant?: "solid" | "ghost";
  style?: ViewStyle;
}) {
  const solid = props.variant !== "ghost";
  return (
    <Pressable
      accessibilityRole="button"
      disabled={props.disabled || props.loading}
      onPress={props.onPress}
      style={({ pressed }) => [
        styles.button,
        solid ? { backgroundColor: toneColor(props.tone) } : styles.ghostButton,
        (props.disabled || props.loading) && styles.disabled,
        pressed && styles.pressed,
        props.style,
      ]}
    >
      {props.loading ? (
        <ActivityIndicator size="small" color={solid ? "#fff" : toneColor(props.tone)} />
      ) : (
        <Text style={[styles.buttonText, !solid && { color: toneColor(props.tone) }]}>
          {props.title}
        </Text>
      )}
    </Pressable>
  );
}

export function ListItem(props: {
  title: string;
  subtitle?: string;
  left?: React.ReactNode;
  right?: React.ReactNode;
  onPress?: () => void;
}) {
  const body = (
    <View style={styles.listItem}>
      {props.left ? <View style={styles.listLeft}>{props.left}</View> : null}
      <View style={{ flex: 1 }}>
        <Text style={styles.listTitle} numberOfLines={1}>
          {props.title}
        </Text>
        {props.subtitle ? (
          <Text style={styles.listSubtitle} numberOfLines={2}>
            {props.subtitle}
          </Text>
        ) : null}
      </View>
      {props.right}
    </View>
  );
  if (!props.onPress) return body;
  return (
    <Pressable onPress={props.onPress} style={({ pressed }) => pressed && styles.pressed}>
      {body}
    </Pressable>
  );
}

export function StatusDot(props: { tone?: Tone; label?: string }) {
  return (
    <View style={styles.statusWrap}>
      <View style={[styles.statusDot, { backgroundColor: toneColor(props.tone) }]} />
      {props.label ? <Text style={styles.statusLabel}>{props.label}</Text> : null}
    </View>
  );
}

export function Switch(props: { value: boolean; onValueChange?: (value: boolean) => void; disabled?: boolean }) {
  return (
    <Pressable
      accessibilityRole="switch"
      accessibilityState={{ checked: props.value, disabled: props.disabled }}
      disabled={props.disabled}
      onPress={() => props.onValueChange?.(!props.value)}
      style={[
        styles.switchTrack,
        props.value ? styles.switchOn : styles.switchOff,
        props.disabled && styles.disabled,
      ]}
    >
      <View style={[styles.switchThumb, props.value && styles.switchThumbOn]} />
    </Pressable>
  );
}

export function TextField(props: TextInputProps) {
  return <TextInput {...props} placeholderTextColor={uiColors.faint} style={[styles.input, props.style as TextStyle]} />;
}

export function ImagePreviewModal(props: { uri: string | null; visible: boolean; onClose: () => void }) {
  return (
    <Modal transparent visible={props.visible} animationType="fade" onRequestClose={props.onClose}>
      <Pressable style={styles.previewBackdrop} onPress={props.onClose}>
        {props.uri ? <Image source={{ uri: props.uri }} style={styles.previewImage} resizeMode="contain" /> : null}
      </Pressable>
    </Modal>
  );
}

export function ScrollPanel(props: { children: React.ReactNode; style?: ViewStyle }) {
  return (
    <ScrollView style={props.style} contentContainerStyle={styles.scrollPanel}>
      {props.children}
    </ScrollView>
  );
}

function toneColor(tone: Tone = "accent") {
  switch (tone) {
    case "neutral":
      return uiColors.coal;
    case "success":
      return uiColors.success;
    case "warning":
      return uiColors.warning;
    case "danger":
      return uiColors.danger;
    case "accent":
    default:
      return uiColors.accent;
  }
}

const styles = StyleSheet.create({
  text: {
    color: uiColors.ink,
    fontFamily: "IBMPlexSans-Regular",
    fontSize: 14,
    lineHeight: 20,
  },
  surface: {
    backgroundColor: uiColors.panel,
    borderColor: uiColors.line,
    borderRadius: 8,
    borderWidth: StyleSheet.hairlineWidth,
  },
  sectionHeader: {
    alignItems: "center",
    flexDirection: "row",
    gap: 12,
    paddingHorizontal: 4,
    paddingVertical: 10,
  },
  sectionTitle: {
    fontFamily: "IBMPlexSans-SemiBold",
    fontSize: 15,
  },
  sectionDetail: {
    color: uiColors.muted,
    fontSize: 12,
  },
  iconButton: {
    alignItems: "center",
    backgroundColor: "#EDF2F6",
    borderRadius: 6,
    height: 32,
    justifyContent: "center",
    width: 32,
  },
  button: {
    alignItems: "center",
    borderRadius: 6,
    minHeight: 34,
    justifyContent: "center",
    paddingHorizontal: 14,
  },
  buttonText: {
    color: "#FFFFFF",
    fontFamily: "IBMPlexSans-SemiBold",
  },
  ghostButton: {
    backgroundColor: "transparent",
    borderColor: uiColors.line,
    borderWidth: StyleSheet.hairlineWidth,
  },
  disabled: {
    opacity: 0.45,
  },
  pressed: {
    opacity: 0.72,
  },
  listItem: {
    alignItems: "center",
    flexDirection: "row",
    gap: 12,
    minHeight: 56,
    padding: 12,
  },
  listLeft: {
    alignItems: "center",
    justifyContent: "center",
    width: 28,
  },
  listTitle: {
    fontFamily: "IBMPlexSans-SemiBold",
  },
  listSubtitle: {
    color: uiColors.muted,
    fontSize: 12,
    lineHeight: 17,
  },
  statusWrap: {
    alignItems: "center",
    flexDirection: "row",
    gap: 6,
  },
  statusDot: {
    borderRadius: 4,
    height: 8,
    width: 8,
  },
  statusLabel: {
    color: uiColors.muted,
    fontSize: 12,
  },
  switchTrack: {
    borderRadius: 999,
    height: 24,
    padding: 2,
    width: 42,
  },
  switchOn: {
    backgroundColor: uiColors.accent,
  },
  switchOff: {
    backgroundColor: "#CAD3DB",
  },
  switchThumb: {
    backgroundColor: "#FFFFFF",
    borderRadius: 10,
    height: 20,
    width: 20,
  },
  switchThumbOn: {
    transform: [{ translateX: 18 }],
  },
  input: {
    backgroundColor: "#FFFFFF",
    borderColor: uiColors.line,
    borderRadius: 6,
    borderWidth: StyleSheet.hairlineWidth,
    color: uiColors.ink,
    fontFamily: "IBMPlexSans-Regular",
    minHeight: 38,
    paddingHorizontal: 10,
  },
  previewBackdrop: {
    alignItems: "center",
    backgroundColor: "rgba(0,0,0,0.82)",
    flex: 1,
    justifyContent: "center",
    padding: 24,
  },
  previewImage: {
    height: "100%",
    width: "100%",
  },
  scrollPanel: {
    gap: 10,
    padding: 12,
  },
});
