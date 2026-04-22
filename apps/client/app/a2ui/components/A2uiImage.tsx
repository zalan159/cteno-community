import React from 'react';
import { View, Image } from 'react-native';
import { useUnistyles } from 'react-native-unistyles';
import { Text } from '@/components/StyledText';
import { Typography } from '@/constants/Typography';

interface Props {
  src: string;
  alt?: string;
  caption?: string;
}

export function A2uiImage({ src, alt, caption }: Props) {
  const { theme } = useUnistyles();
  return (
    <View style={{ gap: 4 }}>
      <Image
        source={{ uri: src }}
        style={{ width: '100%', height: 200, borderRadius: 8 }}
        resizeMode="cover"
        accessibilityLabel={alt}
      />
      {caption ? (
        <Text style={{ fontSize: 12, color: theme.colors.textSecondary, textAlign: 'center', ...Typography.default() }}>
          {caption}
        </Text>
      ) : null}
    </View>
  );
}
