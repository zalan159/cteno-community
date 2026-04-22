import React from 'react';
import { View } from 'react-native';
import { useUnistyles } from 'react-native-unistyles';
import { Text } from '@/components/StyledText';
import { Typography } from '@/constants/Typography';

interface Props {
  title?: string;
  renderChildren?: () => React.ReactNode;
}

export function A2uiCard({ title, renderChildren }: Props) {
  const { theme } = useUnistyles();
  return (
    <View
      style={{
        backgroundColor: theme.colors.surfaceHighest,
        borderRadius: 12,
        borderWidth: 1,
        borderColor: theme.colors.divider,
        padding: 16,
        gap: 12,
      }}
    >
      {title ? (
        <Text style={{ fontSize: 15, color: theme.colors.text, ...Typography.default('semiBold') }}>
          {title}
        </Text>
      ) : null}
      {renderChildren?.()}
    </View>
  );
}
