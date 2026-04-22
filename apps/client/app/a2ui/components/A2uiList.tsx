import React from 'react';
import { View } from 'react-native';
import { useUnistyles } from 'react-native-unistyles';
import { Text } from '@/components/StyledText';
import { Typography } from '@/constants/Typography';

interface Props {
  title?: string;
  renderChildren?: () => React.ReactNode;
}

export function A2uiList({ title, renderChildren }: Props) {
  const { theme } = useUnistyles();
  return (
    <View style={{ gap: 0 }}>
      {title ? (
        <Text style={{ fontSize: 13, color: theme.colors.textSecondary, marginBottom: 6, ...Typography.default('semiBold') }}>
          {title}
        </Text>
      ) : null}
      {renderChildren?.()}
    </View>
  );
}
