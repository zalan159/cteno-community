import * as React from 'react';
import { ActivityIndicator, TextInput, TouchableOpacity, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { sessionRespondToElicitation } from '@/sync/ops';
import { ToolSectionView } from '../../tools/ToolSectionView';
import { CommandView } from '@/components/CommandView';
import { knownTools } from '@/components/tools/knownTools';
import { Text } from '@/components/StyledText';
import { ToolViewProps } from './_all';

type TerminalInteraction = {
    prompt: string | null;
    placeholder: string | null;
    requestId: string | null;
};

function pickString(value: unknown): string | null {
    return typeof value === 'string' && value.trim().length > 0 ? value : null;
}

function getTerminalInteraction(value: unknown): TerminalInteraction | null {
    if (typeof value === 'string' && value.trim().length > 0) {
        return {
            prompt: value,
            placeholder: null,
            requestId: null,
        };
    }
    if (!value || typeof value !== 'object') {
        return null;
    }

    const input = value as Record<string, unknown>;
    return {
        prompt:
            pickString(input.prompt)
            ?? pickString(input.message)
            ?? pickString(input.description)
            ?? pickString(input.question)
            ?? pickString(input.label)
            ?? pickString(input.text),
        placeholder:
            pickString(input.placeholder)
            ?? pickString(input.hint)
            ?? pickString(input.defaultInput),
        requestId:
            pickString(input.requestId)
            ?? pickString(input.request_id)
            ?? pickString(input.id),
    };
}

const styles = StyleSheet.create((theme) => ({
    interactionContainer: {
        gap: 10,
    },
    interactionPrompt: {
        fontSize: 13,
        lineHeight: 18,
        color: theme.colors.textSecondary,
    },
    interactionInput: {
        minHeight: 42,
        borderWidth: 1,
        borderColor: theme.colors.divider,
        borderRadius: 8,
        backgroundColor: theme.colors.surface,
        color: theme.colors.text,
        paddingHorizontal: 12,
        paddingVertical: 10,
        fontSize: 14,
    },
    interactionInputDisabled: {
        opacity: 0.6,
    },
    interactionActions: {
        flexDirection: 'row',
        justifyContent: 'flex-end',
    },
    interactionSubmit: {
        minWidth: 88,
        minHeight: 36,
        paddingHorizontal: 14,
        borderRadius: 8,
        alignItems: 'center',
        justifyContent: 'center',
        backgroundColor: theme.colors.button.primary.background,
    },
    interactionSubmitDisabled: {
        opacity: 0.5,
    },
    interactionSubmitText: {
        fontSize: 13,
        fontWeight: '600',
        color: theme.colors.button.primary.tint,
    },
}));

export const BashView = React.memo<ToolViewProps>(({ tool, sessionId }) => {
    const { theme } = useUnistyles();
    const { input, result, state } = tool;
    const terminalInteraction = getTerminalInteraction(input?.terminalInteraction);
    const [stdinValue, setStdinValue] = React.useState('');
    const [isSubmitting, setIsSubmitting] = React.useState(false);

    React.useEffect(() => {
        setStdinValue('');
        setIsSubmitting(false);
    }, [tool.callId, terminalInteraction?.requestId, terminalInteraction?.prompt]);

    let parsedResult: { stdout?: string; stderr?: string } | null = null;
    let unparsedOutput: string | null = null;
    let error: string | null = null;
    const streamingOutput = {
        stdout: pickString(input?.stdout),
        stderr: pickString(input?.stderr),
    };

    if (state === 'completed' && result) {
        if (typeof result === 'string') {
            unparsedOutput = result;
        } else {
            const parsed = knownTools.Bash.result.safeParse(result);
            if (parsed.success) {
                parsedResult = parsed.data;
            } else {
                unparsedOutput = JSON.stringify(result);
            }
        }
    } else if (state === 'error' && typeof result === 'string') {
        error = result;
    }

    const canSubmitInteraction =
        state === 'running'
        && !!sessionId
        && !!tool.callId
        && !!terminalInteraction
        && !isSubmitting;

    const handleSubmit = React.useCallback(async () => {
        if (!sessionId || !tool.callId || !terminalInteraction || isSubmitting) {
            return;
        }

        setIsSubmitting(true);
        try {
            await sessionRespondToElicitation(
                sessionId,
                terminalInteraction.requestId ?? tool.callId,
                {
                    action: 'accept',
                    content: {
                        text: stdinValue,
                    },
                }
            );
            setStdinValue('');
        } catch (submitError) {
            console.error('Failed to submit Codex terminal input:', submitError);
        } finally {
            setIsSubmitting(false);
        }
    }, [isSubmitting, sessionId, stdinValue, terminalInteraction, tool.callId]);

    return (
        <>
            <ToolSectionView>
                <CommandView
                    command={typeof input?.command === 'string' ? input.command : ''}
                    stdout={parsedResult?.stdout ?? streamingOutput.stdout}
                    stderr={parsedResult?.stderr ?? streamingOutput.stderr}
                    error={error}
                    hideEmptyOutput={
                        !parsedResult
                        && !unparsedOutput
                        && !streamingOutput.stdout
                        && !streamingOutput.stderr
                    }
                />
            </ToolSectionView>

            {unparsedOutput ? (
                <ToolSectionView>
                    <Text>{unparsedOutput}</Text>
                </ToolSectionView>
            ) : null}

            {state === 'running' && terminalInteraction ? (
                <ToolSectionView title="Stdin">
                    <View style={styles.interactionContainer}>
                        {terminalInteraction.prompt ? (
                            <Text style={styles.interactionPrompt}>{terminalInteraction.prompt}</Text>
                        ) : null}
                        <TextInput
                            style={[
                                styles.interactionInput,
                                !canSubmitInteraction && styles.interactionInputDisabled,
                            ]}
                            value={stdinValue}
                            onChangeText={setStdinValue}
                            editable={canSubmitInteraction}
                            placeholder={terminalInteraction.placeholder ?? 'Send stdin to Codex'}
                            placeholderTextColor={theme.colors.textSecondary}
                            autoCorrect={false}
                            autoCapitalize="none"
                            onSubmitEditing={() => {
                                void handleSubmit();
                            }}
                        />
                        <View style={styles.interactionActions}>
                            <TouchableOpacity
                                style={[
                                    styles.interactionSubmit,
                                    (!canSubmitInteraction || stdinValue.length === 0) && styles.interactionSubmitDisabled,
                                ]}
                                onPress={() => {
                                    void handleSubmit();
                                }}
                                disabled={!canSubmitInteraction || stdinValue.length === 0}
                                activeOpacity={0.7}
                            >
                                {isSubmitting ? (
                                    <ActivityIndicator size="small" color={theme.colors.button.primary.tint} />
                                ) : (
                                    <Text style={styles.interactionSubmitText}>Send</Text>
                                )}
                            </TouchableOpacity>
                        </View>
                    </View>
                </ToolSectionView>
            ) : null}
        </>
    );
});
