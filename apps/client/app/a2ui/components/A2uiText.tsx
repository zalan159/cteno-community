import React from 'react';
import { useUnistyles } from 'react-native-unistyles';
import { Text } from '@/components/StyledText';
import { Typography } from '@/constants/Typography';
import { MarkdownView } from '@/components/markdown/MarkdownView';

type Variant = 'heading' | 'subheading' | 'body' | 'caption' | 'code';

interface Props {
  text: string;
  variant?: Variant;
  markdown?: boolean;
}

const variantStyles: Record<Variant, { fontSize: number; weight?: 'semiBold' }> = {
  heading: { fontSize: 20, weight: 'semiBold' },
  subheading: { fontSize: 16, weight: 'semiBold' },
  body: { fontSize: 14 },
  caption: { fontSize: 12 },
  code: { fontSize: 13 },
};

export function A2uiText({ text, variant = 'body', markdown }: Props) {
  const { theme } = useUnistyles();

  if (markdown) {
    return <MarkdownView markdown={text} />;
  }

  const style = variantStyles[variant];
  return (
    <Text
      style={{
        fontSize: style.fontSize,
        color: variant === 'caption' ? theme.colors.textSecondary : theme.colors.text,
        ...Typography.default(style.weight),
        ...(variant === 'code' ? { fontFamily: 'monospace' } : {}),
      }}
    >
      {text}
    </Text>
  );
}
