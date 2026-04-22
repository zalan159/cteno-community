/**
 * A2uiView — wrapper component that combines useA2uiState hook with A2uiRenderer.
 * Drop-in replacement for the WebView-based AIUI rendering.
 */
import React from 'react';
import { View, ActivityIndicator, ScrollView } from 'react-native';
import { useUnistyles } from 'react-native-unistyles';
import { Text } from '@/components/StyledText';
import { useA2uiState } from '@/hooks/useA2uiState';
import { A2uiRenderer } from '@/a2ui/A2uiRenderer';
import { machineA2uiAction } from '@/sync/ops';
import type { A2uiActionEvent } from '@/a2ui/types';

interface A2uiViewProps {
  machineId: string;
  agentId: string;
}

export function A2uiView({ machineId, agentId }: A2uiViewProps) {
  const { theme } = useUnistyles();
  const { surfaces, loading } = useA2uiState({ machineId, agentId });

  const handleAction = React.useCallback(
    (event: A2uiActionEvent) => {
      machineA2uiAction(machineId, agentId, event.surfaceId, event.componentId, event.event);
    },
    [machineId, agentId],
  );

  if (loading && surfaces.length === 0) {
    return (
      <View style={{ flex: 1, justifyContent: 'center', alignItems: 'center', backgroundColor: theme.colors.surface }}>
        <ActivityIndicator size="large" color={theme.colors.textSecondary} />
      </View>
    );
  }

  return (
    <ScrollView
      style={{ flex: 1, backgroundColor: theme.colors.surface }}
      contentContainerStyle={{ flexGrow: 1 }}
    >
      <A2uiRenderer surfaces={surfaces} onAction={handleAction} />
    </ScrollView>
  );
}
