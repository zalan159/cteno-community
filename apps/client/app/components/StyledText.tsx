import React from 'react';
import { Text as RNText, TextProps as RNTextProps } from 'react-native';
import { Typography } from '@/constants/Typography';

interface StyledTextProps extends RNTextProps {
  /**
   * Whether to use the default typography. Set to false to skip default font.
   * Useful when you want to use a different typography style.
   */
  useDefaultTypography?: boolean;
  /**
   * Whether the text should be selectable. Defaults to false.
   */
  selectable?: boolean;
}

/**
 * App Text wrapper.
 *
 * Keep this intentionally minimal: avoid flattening styles here because many call
 * sites pass Unistyles-managed style objects/IDs which need to resolve at render time.
 */
export const Text = React.forwardRef<React.ElementRef<typeof RNText>, StyledTextProps>(
  ({ style, useDefaultTypography = true, selectable = false, ...props }, ref) => {
    const defaultStyle = useDefaultTypography ? Typography.default() : undefined;
    return <RNText ref={ref} style={[defaultStyle, style]} selectable={selectable} {...props} />;
  }
);
Text.displayName = 'Text';

// Export the original RNText as well, in case it's needed
export { Text as RNText } from 'react-native';

