import * as React from 'react';
import { ActivityIndicator, TextInput, TouchableOpacity, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Text } from '@/components/StyledText';
import { sessionRespondToElicitation } from '@/sync/ops';
import { ToolSectionView } from '../ToolSectionView';
import { ToolViewProps } from './_all';

type ElicitationOption = {
    const: string;
    title: string;
};

type ElicitationFieldSchema =
    | {
        type: 'string';
        title?: string;
        description?: string;
        enum?: string[];
        oneOf?: ElicitationOption[];
        default?: string;
        minLength?: number;
        maxLength?: number;
        format?: 'date' | 'uri' | 'email' | 'date-time';
    }
    | {
        type: 'array';
        title?: string;
        description?: string;
        minItems?: number;
        maxItems?: number;
        items?: {
            type?: 'string';
            enum?: string[];
            anyOf?: ElicitationOption[];
        };
        default?: string[];
    }
    | {
        type: 'boolean';
        title?: string;
        description?: string;
        default?: boolean;
    }
    | {
        type: 'number' | 'integer';
        title?: string;
        description?: string;
        minimum?: number;
        maximum?: number;
        default?: number;
    };

type ElicitationInput = {
    serverName?: string;
    message?: string;
    mode?: string;
    url?: string;
    elicitationId?: string;
    requestedSchema?: {
        type?: 'object';
        properties?: Record<string, ElicitationFieldSchema>;
        required?: string[];
    };
    title?: string;
    displayName?: string;
    description?: string;
};

type FormValue = string | boolean | string[];

function getFieldOptions(field: ElicitationFieldSchema): ElicitationOption[] | null {
    if (field.type === 'string') {
        if (Array.isArray(field.oneOf) && field.oneOf.length > 0) {
            return field.oneOf;
        }
        if (Array.isArray(field.enum) && field.enum.length > 0) {
            return field.enum.map((value) => ({ const: value, title: value }));
        }
    }
    if (field.type === 'array') {
        const anyOf = field.items?.anyOf;
        if (Array.isArray(anyOf) && anyOf.length > 0) {
            return anyOf;
        }
        const enumValues = field.items?.enum;
        if (Array.isArray(enumValues) && enumValues.length > 0) {
            return enumValues.map((value) => ({ const: value, title: value }));
        }
    }
    return null;
}

function defaultFieldValue(field: ElicitationFieldSchema): FormValue {
    if (field.type === 'boolean') {
        return field.default ?? false;
    }
    if (field.type === 'array') {
        return Array.isArray(field.default) ? field.default : [];
    }
    if (field.type === 'number' || field.type === 'integer') {
        return field.default != null ? String(field.default) : '';
    }
    return typeof field.default === 'string' ? field.default : '';
}

function buildInitialValues(properties: Record<string, ElicitationFieldSchema>): Record<string, FormValue> {
    return Object.fromEntries(
        Object.entries(properties).map(([name, field]) => [name, defaultFieldValue(field)])
    );
}

function isEmptyValue(value: FormValue | undefined): boolean {
    if (Array.isArray(value)) {
        return value.length === 0;
    }
    if (typeof value === 'boolean') {
        return false;
    }
    return !value || value.trim().length === 0;
}

function buildErrors(
    properties: Record<string, ElicitationFieldSchema>,
    required: string[],
    values: Record<string, FormValue>
): Record<string, string> {
    const errors: Record<string, string> = {};

    for (const [name, field] of Object.entries(properties)) {
        const value = values[name];

        if (required.includes(name) && isEmptyValue(value)) {
            errors[name] = 'Required';
            continue;
        }
        if (value == null || isEmptyValue(value)) {
            continue;
        }

        if (field.type === 'string' && typeof value === 'string') {
            if (field.minLength != null && value.length < field.minLength) {
                errors[name] = `Must be at least ${field.minLength} characters`;
            } else if (field.maxLength != null && value.length > field.maxLength) {
                errors[name] = `Must be at most ${field.maxLength} characters`;
            }
        }

        if (field.type === 'array' && Array.isArray(value)) {
            if (field.minItems != null && value.length < field.minItems) {
                errors[name] = `Select at least ${field.minItems}`;
            } else if (field.maxItems != null && value.length > field.maxItems) {
                errors[name] = `Select at most ${field.maxItems}`;
            }
        }

        if ((field.type === 'number' || field.type === 'integer') && typeof value === 'string') {
            const parsed = Number(value);
            if (!Number.isFinite(parsed)) {
                errors[name] = 'Enter a valid number';
                continue;
            }
            if (field.type === 'integer' && !Number.isInteger(parsed)) {
                errors[name] = 'Enter a whole number';
                continue;
            }
            if (field.minimum != null && parsed < field.minimum) {
                errors[name] = `Must be at least ${field.minimum}`;
            } else if (field.maximum != null && parsed > field.maximum) {
                errors[name] = `Must be at most ${field.maximum}`;
            }
        }
    }

    return errors;
}

function buildResponseContent(
    properties: Record<string, ElicitationFieldSchema>,
    values: Record<string, FormValue>
): Record<string, unknown> {
    const content: Record<string, unknown> = {};

    for (const [name, field] of Object.entries(properties)) {
        const value = values[name];
        if (value == null || isEmptyValue(value)) {
            continue;
        }
        if (field.type === 'boolean' && typeof value === 'boolean') {
            content[name] = value;
            continue;
        }
        if (field.type === 'array' && Array.isArray(value)) {
            content[name] = value;
            continue;
        }
        if ((field.type === 'number' || field.type === 'integer') && typeof value === 'string') {
            content[name] = Number(value);
            continue;
        }
        content[name] = value;
    }

    return content;
}

const styles = StyleSheet.create((theme) => ({
    container: {
        gap: 14,
    },
    header: {
        gap: 6,
    },
    title: {
        fontSize: 15,
        fontWeight: '600',
        color: theme.colors.text,
    },
    description: {
        fontSize: 13,
        color: theme.colors.textSecondary,
        lineHeight: 18,
    },
    fieldSection: {
        gap: 8,
    },
    fieldTitle: {
        fontSize: 13,
        fontWeight: '600',
        color: theme.colors.text,
    },
    requiredMark: {
        color: theme.colors.warning,
    },
    fieldDescription: {
        fontSize: 12,
        color: theme.colors.textSecondary,
        lineHeight: 17,
    },
    input: {
        borderWidth: 1,
        borderColor: theme.colors.divider,
        borderRadius: 8,
        backgroundColor: theme.colors.surface,
        color: theme.colors.text,
        paddingHorizontal: 12,
        paddingVertical: 10,
        minHeight: 44,
        fontSize: 14,
    },
    optionsContainer: {
        gap: 6,
    },
    optionButton: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 10,
        paddingHorizontal: 12,
        paddingVertical: 12,
        borderRadius: 8,
        borderWidth: 1,
        borderColor: theme.colors.divider,
        backgroundColor: theme.colors.surface,
    },
    optionButtonSelected: {
        borderColor: theme.colors.button.primary.background,
        backgroundColor: theme.colors.surfaceHigh,
    },
    optionIndicator: {
        width: 18,
        height: 18,
        borderRadius: 9,
        borderWidth: 2,
        borderColor: theme.colors.textSecondary,
    },
    optionIndicatorSquare: {
        borderRadius: 4,
    },
    optionIndicatorSelected: {
        borderColor: theme.colors.button.primary.background,
        backgroundColor: theme.colors.button.primary.background,
    },
    optionText: {
        flex: 1,
        fontSize: 14,
        color: theme.colors.text,
    },
    errorText: {
        fontSize: 12,
        color: theme.colors.warning,
    },
    actions: {
        flexDirection: 'row',
        justifyContent: 'flex-end',
        marginTop: 4,
    },
    submitButton: {
        minHeight: 44,
        paddingHorizontal: 18,
        paddingVertical: 12,
        borderRadius: 8,
        backgroundColor: theme.colors.button.primary.background,
        alignItems: 'center',
        justifyContent: 'center',
    },
    submitButtonDisabled: {
        opacity: 0.5,
    },
    submitButtonText: {
        fontSize: 14,
        fontWeight: '600',
        color: theme.colors.button.primary.tint,
    },
    submittedContainer: {
        gap: 8,
    },
    submittedRow: {
        gap: 2,
    },
    submittedKey: {
        fontSize: 12,
        fontWeight: '600',
        color: theme.colors.textSecondary,
        textTransform: 'uppercase',
    },
    submittedValue: {
        fontSize: 13,
        color: theme.colors.text,
    },
}));

export const ElicitationView = React.memo<ToolViewProps>(({ tool, sessionId }) => {
    const { theme } = useUnistyles();
    const input = (tool.input ?? {}) as ElicitationInput;
    const properties = input.requestedSchema?.properties ?? {};
    const required = input.requestedSchema?.required ?? [];
    const fieldEntries = Object.entries(properties);

    const [values, setValues] = React.useState<Record<string, FormValue>>(() => buildInitialValues(properties));
    const [errors, setErrors] = React.useState<Record<string, string>>({});
    const [isSubmitting, setIsSubmitting] = React.useState(false);
    const [isSubmitted, setIsSubmitted] = React.useState(false);

    React.useEffect(() => {
        setValues(buildInitialValues(properties));
        setErrors({});
        setIsSubmitting(false);
        setIsSubmitted(false);
    }, [tool.callId, input.requestedSchema]);

    const canInteract = tool.state === 'running' && !isSubmitted;

    const setFieldValue = (name: string, value: FormValue) => {
        setValues((current) => ({ ...current, [name]: value }));
        setErrors((current) => {
            if (!current[name]) {
                return current;
            }
            const next = { ...current };
            delete next[name];
            return next;
        });
    };

    const handleSubmit = async () => {
        if (!sessionId || !tool.callId || isSubmitting) {
            return;
        }

        const nextErrors = buildErrors(properties, required, values);
        setErrors(nextErrors);
        if (Object.keys(nextErrors).length > 0) {
            return;
        }

        setIsSubmitting(true);
        try {
            const content = buildResponseContent(properties, values);
            await sessionRespondToElicitation(sessionId, tool.callId, {
                action: 'accept',
                ...(Object.keys(content).length > 0 ? { content } : {}),
            });
            setIsSubmitted(true);
        } catch (error) {
            console.error('Failed to submit elicitation response:', error);
        } finally {
            setIsSubmitting(false);
        }
    };

    if (isSubmitted) {
        const submittedContent = buildResponseContent(properties, values);
        return (
            <ToolSectionView>
                <View style={styles.submittedContainer}>
                    {Object.entries(submittedContent).map(([name, value]) => (
                        <View key={name} style={styles.submittedRow}>
                            <Text style={styles.submittedKey}>{name}</Text>
                            <Text style={styles.submittedValue}>
                                {Array.isArray(value) ? value.join(', ') : String(value)}
                            </Text>
                        </View>
                    ))}
                </View>
            </ToolSectionView>
        );
    }

    return (
        <ToolSectionView>
            <View style={styles.container}>
                <View style={styles.header}>
                    <Text style={styles.title}>{input.title || input.displayName || 'Input Required'}</Text>
                    {input.description ? <Text style={styles.description}>{input.description}</Text> : null}
                    {input.message ? <Text style={styles.description}>{input.message}</Text> : null}
                </View>

                {fieldEntries.map(([name, field]) => {
                    const value = values[name];
                    const options = getFieldOptions(field);
                    const title = field.title || name;

                    return (
                        <View key={name} style={styles.fieldSection}>
                            <Text style={styles.fieldTitle}>
                                {title}
                                {required.includes(name) ? <Text style={styles.requiredMark}> *</Text> : null}
                            </Text>
                            {field.description ? <Text style={styles.fieldDescription}>{field.description}</Text> : null}

                            {field.type === 'string' && options ? (
                                <View style={styles.optionsContainer}>
                                    {options.map((option) => {
                                        const selected = value === option.const;
                                        return (
                                            <TouchableOpacity
                                                key={option.const}
                                                style={[
                                                    styles.optionButton,
                                                    selected && styles.optionButtonSelected,
                                                ]}
                                                onPress={() => setFieldValue(name, option.const)}
                                                disabled={!canInteract}
                                                activeOpacity={0.7}
                                            >
                                                <View style={[
                                                    styles.optionIndicator,
                                                    selected && styles.optionIndicatorSelected,
                                                ]} />
                                                <Text style={styles.optionText}>{option.title}</Text>
                                            </TouchableOpacity>
                                        );
                                    })}
                                </View>
                            ) : null}

                            {field.type === 'array' && options ? (
                                <View style={styles.optionsContainer}>
                                    {options.map((option) => {
                                        const selectedValues = Array.isArray(value) ? value : [];
                                        const selected = selectedValues.includes(option.const);
                                        return (
                                            <TouchableOpacity
                                                key={option.const}
                                                style={[
                                                    styles.optionButton,
                                                    selected && styles.optionButtonSelected,
                                                ]}
                                                onPress={() => {
                                                    const current = Array.isArray(value) ? value : [];
                                                    setFieldValue(
                                                        name,
                                                        selected
                                                            ? current.filter((item) => item !== option.const)
                                                            : [...current, option.const]
                                                    );
                                                }}
                                                disabled={!canInteract}
                                                activeOpacity={0.7}
                                            >
                                                <View style={[
                                                    styles.optionIndicator,
                                                    styles.optionIndicatorSquare,
                                                    selected && styles.optionIndicatorSelected,
                                                ]} />
                                                <Text style={styles.optionText}>{option.title}</Text>
                                            </TouchableOpacity>
                                        );
                                    })}
                                </View>
                            ) : null}

                            {field.type === 'boolean' ? (
                                <View style={styles.optionsContainer}>
                                    {[
                                        { label: 'Yes', value: true },
                                        { label: 'No', value: false },
                                    ].map((option) => {
                                        const selected = value === option.value;
                                        return (
                                            <TouchableOpacity
                                                key={option.label}
                                                style={[
                                                    styles.optionButton,
                                                    selected && styles.optionButtonSelected,
                                                ]}
                                                onPress={() => setFieldValue(name, option.value)}
                                                disabled={!canInteract}
                                                activeOpacity={0.7}
                                            >
                                                <View style={[
                                                    styles.optionIndicator,
                                                    selected && styles.optionIndicatorSelected,
                                                ]} />
                                                <Text style={styles.optionText}>{option.label}</Text>
                                            </TouchableOpacity>
                                        );
                                    })}
                                </View>
                            ) : null}

                            {field.type === 'string' && !options ? (
                                <TextInput
                                    style={[
                                        styles.input,
                                        errors[name] ? { borderColor: theme.colors.warning } : null,
                                    ]}
                                    value={typeof value === 'string' ? value : ''}
                                    onChangeText={(text) => setFieldValue(name, text)}
                                    editable={canInteract}
                                    autoCapitalize="none"
                                    autoCorrect={false}
                                    keyboardType={
                                        field.format === 'email'
                                            ? 'email-address'
                                            : field.format === 'uri'
                                                ? 'url'
                                                : 'default'
                                    }
                                />
                            ) : null}

                            {(field.type === 'number' || field.type === 'integer') ? (
                                <TextInput
                                    style={[
                                        styles.input,
                                        errors[name] ? { borderColor: theme.colors.warning } : null,
                                    ]}
                                    value={typeof value === 'string' ? value : ''}
                                    onChangeText={(text) => setFieldValue(name, text)}
                                    editable={canInteract}
                                    keyboardType="numeric"
                                />
                            ) : null}

                            {errors[name] ? <Text style={styles.errorText}>{errors[name]}</Text> : null}
                        </View>
                    );
                })}

                <View style={styles.actions}>
                    <TouchableOpacity
                        style={[
                            styles.submitButton,
                            (isSubmitting || !canInteract) && styles.submitButtonDisabled,
                        ]}
                        onPress={handleSubmit}
                        disabled={isSubmitting || !canInteract}
                        activeOpacity={0.7}
                    >
                        {isSubmitting ? (
                            <ActivityIndicator size="small" color={theme.colors.button.primary.tint} />
                        ) : (
                            <Text style={styles.submitButtonText}>Submit</Text>
                        )}
                    </TouchableOpacity>
                </View>
            </View>
        </ToolSectionView>
    );
});
