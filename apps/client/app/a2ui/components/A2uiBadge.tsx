import React from 'react';
import { View } from 'react-native';
import { Text } from '@/components/StyledText';
import { Typography } from '@/constants/Typography';

interface Props {
  text: string;
  variant?: 'info' | 'success' | 'warning' | 'error';
}

const variantColors: Record<string, { bg: string; fg: string }> = {
  info: { bg: '#DBEAFE', fg: '#1D4ED8' },
  success: { bg: '#DCFCE7', fg: '#15803D' },
  warning: { bg: '#FEF9C3', fg: '#A16207' },
  error: { bg: '#FEE2E2', fg: '#B91C1C' },
};

export function A2uiBadge({ text, variant = 'info' }: Props) {
  const colors = variantColors[variant] || variantColors.info;

  return (
    <View
      style={{
        backgroundColor: colors.bg,
        paddingHorizontal: 8,
        paddingVertical: 2,
        borderRadius: 10,
        alignSelf: 'flex-start',
      }}
    >
      <Text style={{ fontSize: 11, color: colors.fg, ...Typography.default('semiBold') }}>
        {text}
      </Text>
    </View>
  );
}
