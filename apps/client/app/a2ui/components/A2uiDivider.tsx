import React from 'react';
import { View } from 'react-native';
import { useUnistyles } from 'react-native-unistyles';

export function A2uiDivider() {
  const { theme } = useUnistyles();
  return <View style={{ height: 1, backgroundColor: theme.colors.divider, marginVertical: 4 }} />;
}
