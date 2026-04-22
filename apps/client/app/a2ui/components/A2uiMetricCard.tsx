import React from 'react';
import { View } from 'react-native';
import { useUnistyles } from 'react-native-unistyles';
import { Text } from '@/components/StyledText';
import { Typography } from '@/constants/Typography';

interface Props {
  value: string | number;
  label: string;
  trend?: string;
  trendDirection?: 'up' | 'down';
}

export function A2uiMetricCard({ value, label, trend, trendDirection }: Props) {
  const { theme } = useUnistyles();
  const trendColor = trendDirection === 'up' ? '#22C55E' : trendDirection === 'down' ? '#EF4444' : theme.colors.textSecondary;

  return (
    <View
      style={{
        backgroundColor: theme.colors.surface,
        borderWidth: 1,
        borderColor: theme.colors.divider,
        borderRadius: 8,
        padding: 12,
        alignItems: 'center',
        minWidth: 100,
        flex: 1,
      }}
    >
      <Text style={{ fontSize: 20, color: theme.colors.text, ...Typography.default('semiBold') }}>
        {String(value)}
      </Text>
      <Text style={{ fontSize: 11, color: theme.colors.textSecondary, marginTop: 2, ...Typography.default() }}>
        {label}
      </Text>
      {trend ? (
        <Text style={{ fontSize: 11, color: trendColor, marginTop: 2, ...Typography.default() }}>
          {trend}
        </Text>
      ) : null}
    </View>
  );
}
