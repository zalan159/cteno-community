import React from 'react';
import { View } from 'react-native';

interface Props {
  gap?: number;
  align?: 'flex-start' | 'center' | 'flex-end' | 'stretch';
  renderChildren?: () => React.ReactNode;
}

export function A2uiColumn({ gap = 8, align = 'stretch', renderChildren }: Props) {
  return (
    <View style={{ gap, alignItems: align }}>
      {renderChildren?.()}
    </View>
  );
}
