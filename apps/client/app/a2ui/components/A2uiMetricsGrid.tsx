import React from 'react';
import { View } from 'react-native';
import { A2uiMetricCard } from './A2uiMetricCard';

interface Props {
  metrics: Record<string, string | number>;
}

export function A2uiMetricsGrid({ metrics }: Props) {
  if (!metrics || typeof metrics !== 'object') return null;
  const entries = Object.entries(metrics);

  return (
    <View style={{ flexDirection: 'row', flexWrap: 'wrap', gap: 8 }}>
      {entries.map(([label, value]) => (
        <A2uiMetricCard key={label} value={value} label={label} />
      ))}
    </View>
  );
}
