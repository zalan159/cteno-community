import React from 'react';
import { Ionicons } from '@expo/vector-icons';
import { useUnistyles } from 'react-native-unistyles';

interface Props {
  name: string;
  color?: string;
  size?: number;
}

export function A2uiIcon({ name, color, size = 20 }: Props) {
  const { theme } = useUnistyles();
  return <Ionicons name={name as any} size={size} color={color || theme.colors.text} />;
}
