import React from 'react';
import { View } from 'react-native';
import { useUnistyles } from 'react-native-unistyles';
import { Text } from '@/components/StyledText';
import { Typography } from '@/constants/Typography';

interface Props {
  value: number; // 0.0 - 1.0
  label?: string;
}

export function A2uiProgress({ value, label }: Props) {
  const { theme } = useUnistyles();
  const pct = Math.round(Math.max(0, Math.min(1, value)) * 100);

  return (
    <View style={{ gap: 4 }}>
      {label ? (
        <Text style={{ fontSize: 12, color: theme.colors.textSecondary, ...Typography.default() }}>
          {label}
        </Text>
      ) : null}
      <View
        style={{
          height: 8,
          backgroundColor: theme.colors.divider,
          borderRadius: 4,
          overflow: 'hidden',
        }}
      >
        <View
          style={{
            height: '100%',
            width: `${pct}%`,
            backgroundColor: '#3B82F6',
            borderRadius: 4,
          }}
        />
      </View>
    </View>
  );
}
