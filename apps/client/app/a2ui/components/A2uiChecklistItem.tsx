import React from 'react';
import { View } from 'react-native';
import { useUnistyles } from 'react-native-unistyles';
import { Ionicons } from '@expo/vector-icons';
import { Text } from '@/components/StyledText';
import { Typography } from '@/constants/Typography';

interface Props {
  text: string;
  checked: boolean;
}

export function A2uiChecklistItem({ text, checked }: Props) {
  const { theme } = useUnistyles();
  return (
    <View style={{ flexDirection: 'row', alignItems: 'center', gap: 8, paddingVertical: 6 }}>
      <Ionicons
        name={checked ? 'checkbox' : 'square-outline'}
        size={16}
        color={checked ? '#3B82F6' : theme.colors.textSecondary}
      />
      <Text
        style={{
          fontSize: 13,
          color: checked ? theme.colors.textSecondary : theme.colors.text,
          textDecorationLine: checked ? 'line-through' : 'none',
          ...Typography.default(),
        }}
      >
        {text}
      </Text>
    </View>
  );
}
