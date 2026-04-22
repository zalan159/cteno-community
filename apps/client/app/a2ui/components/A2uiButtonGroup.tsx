import React from 'react';
import { View } from 'react-native';

interface Props {
  renderChildren?: () => React.ReactNode;
}

export function A2uiButtonGroup({ renderChildren }: Props) {
  return (
    <View style={{ flexDirection: 'row', gap: 8, flexWrap: 'wrap' }}>
      {renderChildren?.()}
    </View>
  );
}
