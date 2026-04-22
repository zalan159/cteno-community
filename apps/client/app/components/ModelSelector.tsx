import * as React from 'react';
import { View } from 'react-native';
import { useUnistyles } from 'react-native-unistyles';
import { Typography } from '@/constants/Typography';
import type { ModelOptionDisplay } from '@/sync/ops';
import { Text } from '@/components/StyledText';
import { LlmProfileList } from './LlmProfileList';
import { frontendLog } from '@/utils/tauri';

interface ModelSelectorProps {
    models: ModelOptionDisplay[];
    selectedModelId?: string | null;
    defaultModelId?: string;
    onModelChange?: (modelId: string) => void;
    title?: string;
    description?: string;
    emptyMessage?: string;
}

export function ModelSelector({
    models,
    selectedModelId,
    defaultModelId,
    onModelChange,
    title = '模型选择',
    description,
    emptyMessage = '当前机器还没有可用模型。',
}: ModelSelectorProps) {
    const { theme } = useUnistyles();

    React.useEffect(() => {
        frontendLog(`[ModelSelector] ${JSON.stringify({
            count: models.length,
            selectedModelId: selectedModelId ?? null,
            defaultModelId: defaultModelId ?? null,
            ids: models.slice(0, 16).map((model) => ({
                id: model.id,
                model: model.chat?.model,
                isProxy: model.isProxy === true,
            })),
        })}`);
    }, [defaultModelId, models, selectedModelId]);

    return (
        <View>
            {title ? (
                <Text style={{
                    fontSize: 14,
                    fontWeight: '600',
                    color: theme.colors.text,
                    marginBottom: description ? 4 : 8,
                    ...Typography.default('semiBold'),
                }}>
                    {title}
                </Text>
            ) : null}
            {description ? (
                <Text style={{
                    fontSize: 12,
                    color: theme.colors.textSecondary,
                    marginBottom: 8,
                    lineHeight: 18,
                    ...Typography.default(),
                }}>
                    {description}
                </Text>
            ) : null}
            {models.length > 0 ? (
                <LlmProfileList
                    models={models}
                    selectedModelId={selectedModelId ?? undefined}
                    defaultModelId={defaultModelId}
                    onModelChange={onModelChange}
                    variant="modal"
                />
            ) : (
                <Text style={{
                    fontSize: 12,
                    color: theme.colors.textSecondary,
                    ...Typography.default(),
                }}>
                    {emptyMessage}
                </Text>
            )}
        </View>
    );
}
