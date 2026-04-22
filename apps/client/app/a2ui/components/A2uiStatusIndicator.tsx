import React from 'react';
import { View } from 'react-native';
import { useUnistyles } from 'react-native-unistyles';
import { Text } from '@/components/StyledText';
import { Typography } from '@/constants/Typography';

interface Props {
  status: 'active' | 'idle' | 'error';
  text: string;
}

const statusColors: Record<string, string> = {
  active: '#22C55E',
  idle: '#6B7280',
  error: '#EF4444',
};

export function A2uiStatusIndicator({ status, text }: Props) {
  const { theme } = useUnistyles();
  const dotColor = statusColors[status] || statusColors.idle;

  return (
    <View style={{ flexDirection: 'row', alignItems: 'center', gap: 8, paddingVertical: 4 }}>
      <View style={{ width: 8, height: 8, borderRadius: 4, backgroundColor: dotColor }} />
      <Text style={{ fontSize: 13, color: theme.colors.textSecondary, ...Typography.default() }}>
        {text}
      </Text>
    </View>
  );
}
