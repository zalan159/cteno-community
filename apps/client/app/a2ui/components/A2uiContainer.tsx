import React from 'react';
import { View } from 'react-native';

interface Props {
  padding?: number;
  maxWidth?: number;
  background?: string;
  renderChildren?: () => React.ReactNode;
}

export function A2uiContainer({ padding = 16, maxWidth, background, renderChildren }: Props) {
  return (
    <View
      style={{
        flex: 1,
        padding,
        maxWidth,
        backgroundColor: background,
        alignSelf: maxWidth ? 'center' : undefined,
        width: maxWidth ? '100%' : undefined,
      }}
    >
      {renderChildren?.()}
    </View>
  );
}
