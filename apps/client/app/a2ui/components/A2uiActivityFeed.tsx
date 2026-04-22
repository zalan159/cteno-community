import React from 'react';
import { View } from 'react-native';
import { useUnistyles } from 'react-native-unistyles';
import { Text } from '@/components/StyledText';
import { Typography } from '@/constants/Typography';

interface FeedItem {
  text: string;
  timestamp?: string;
}

interface Props {
  items: FeedItem[];
}

export function A2uiActivityFeed({ items }: Props) {
  const { theme } = useUnistyles();
  if (!items || !Array.isArray(items)) return null;

  return (
    <View style={{ gap: 0 }}>
      {items.map((item, i) => (
        <View
          key={i}
          style={{
            flexDirection: 'row',
            alignItems: 'center',
            gap: 8,
            paddingVertical: 6,
            borderBottomWidth: i < items.length - 1 ? 0.5 : 0,
            borderBottomColor: theme.colors.divider,
          }}
        >
          <View style={{ width: 4, height: 4, borderRadius: 2, backgroundColor: '#3B82F6' }} />
          <Text style={{ flex: 1, fontSize: 13, color: theme.colors.text, ...Typography.default() }}>
            {item.text}
          </Text>
          {item.timestamp ? (
            <Text style={{ fontSize: 11, color: theme.colors.textSecondary, ...Typography.default() }}>
              {item.timestamp}
            </Text>
          ) : null}
        </View>
      ))}
    </View>
  );
}
