import React from 'react';
import { View, Pressable } from 'react-native';
import { useUnistyles } from 'react-native-unistyles';
import { Ionicons } from '@expo/vector-icons';
import { Text } from '@/components/StyledText';
import { Typography } from '@/constants/Typography';
import type { A2uiAction } from '../types';

interface Props {
  text: string;
  icon?: string;
  secondaryText?: string;
  action?: A2uiAction;
  onAction?: (action: A2uiAction) => void;
}

export function A2uiListItem({ text, icon, secondaryText, action, onAction }: Props) {
  const { theme } = useUnistyles();

  const content = (
    <View
      style={{
        flexDirection: 'row',
        alignItems: 'center',
        gap: 8,
        paddingVertical: 8,
        borderBottomWidth: 0.5,
        borderBottomColor: theme.colors.divider,
      }}
    >
      {icon ? (
        <Ionicons name={icon as any} size={16} color={theme.colors.textSecondary} />
      ) : (
        <View style={{ width: 4, height: 4, borderRadius: 2, backgroundColor: '#3B82F6' }} />
      )}
      <View style={{ flex: 1 }}>
        <Text style={{ fontSize: 13, color: theme.colors.text, ...Typography.default() }}>{text}</Text>
        {secondaryText ? (
          <Text style={{ fontSize: 11, color: theme.colors.textSecondary, marginTop: 1, ...Typography.default() }}>
            {secondaryText}
          </Text>
        ) : null}
      </View>
      {action ? <Ionicons name="chevron-forward" size={14} color={theme.colors.textSecondary} /> : null}
    </View>
  );

  if (action && onAction) {
    return <Pressable onPress={() => onAction(action)}>{content}</Pressable>;
  }
  return content;
}
