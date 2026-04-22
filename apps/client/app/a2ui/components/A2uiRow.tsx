import React from 'react';
import { View } from 'react-native';

interface Props {
  gap?: number;
  align?: 'flex-start' | 'center' | 'flex-end' | 'stretch';
  justify?: 'flex-start' | 'center' | 'flex-end' | 'space-between' | 'space-around';
  renderChildren?: () => React.ReactNode;
}

export function A2uiRow({ gap = 8, align = 'center', justify, renderChildren }: Props) {
  return (
    <View style={{ flexDirection: 'row', flexWrap: 'wrap', gap, alignItems: align, justifyContent: justify }}>
      {renderChildren?.()}
    </View>
  );
}
