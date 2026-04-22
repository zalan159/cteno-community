import React from 'react';
import { Pressable } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { Text } from '@/components/StyledText';
import { Typography } from '@/constants/Typography';
import type { A2uiAction } from '../types';

interface Props {
  label: string;
  variant?: 'primary' | 'secondary' | 'danger';
  icon?: string;
  action?: A2uiAction;
  onAction?: (action: A2uiAction) => void;
}

const variantStyles = {
  primary: { bg: '#3B82F6', fg: '#FFFFFF' },
  secondary: { bg: '#374151', fg: '#E5E7EB' },
  danger: { bg: '#EF4444', fg: '#FFFFFF' },
};

export function A2uiButton({ label, variant = 'primary', icon, action, onAction }: Props) {
  const style = variantStyles[variant] || variantStyles.primary;

  return (
    <Pressable
      onPress={() => action && onAction?.(action)}
      style={({ pressed }) => ({
        flexDirection: 'row',
        alignItems: 'center',
        justifyContent: 'center',
        gap: 6,
        backgroundColor: style.bg,
        paddingHorizontal: 16,
        paddingVertical: 10,
        borderRadius: 8,
        opacity: pressed ? 0.7 : 1,
      })}
    >
      {icon ? <Ionicons name={icon as any} size={16} color={style.fg} /> : null}
      <Text style={{ fontSize: 14, color: style.fg, ...Typography.default('semiBold') }}>
        {label}
      </Text>
    </Pressable>
  );
}
