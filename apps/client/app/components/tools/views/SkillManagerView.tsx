import * as React from 'react';
import { View, Pressable } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { Ionicons } from '@expo/vector-icons';
import type { ToolViewProps } from './_all';
import { Text } from '@/components/StyledText';
import { t } from '@/text';

export const SkillManagerView = ({ tool }: ToolViewProps) => {
    const [expanded, setExpanded] = React.useState(false);
    const operation = tool.input?.operation as string;

    // Parse the result - handle both string and already-parsed data
    let parsedResult: any = null;

    if (tool.result && typeof tool.result === 'object' && !Array.isArray(tool.result)) {
        // Already parsed as object
        parsedResult = tool.result;
    } else if (tool.result && typeof tool.result === 'string') {
        // Only attempt JSON parse if it looks like JSON (starts with { or [)
        const trimmed = tool.result.trim();
        if (trimmed.startsWith('{') || trimmed.startsWith('[')) {
            try {
                parsedResult = JSON.parse(tool.result);
            } catch (_) {
                // Not valid JSON, will use raw string fallback
            }
        }
    }

    // Simple view for preview operation
    if (operation === 'preview_github' && parsedResult) {
        const { name, version, frontmatter } = parsedResult;
        return (
            <View style={styles.container}>
                <View style={styles.header}>
                    <Ionicons name="eye-outline" size={18} color="#007AFF" />
                    <Text style={styles.headerText}>{t('skills.title')}</Text>
                </View>
                <View style={styles.content}>
                    {name && (
                        <Text style={styles.skillName}>{name}</Text>
                    )}
                    {version && (
                        <Text style={styles.version}>v{version}</Text>
                    )}
                    {frontmatter?.description && (
                        <Text style={styles.description} numberOfLines={3}>
                            {frontmatter.description}
                        </Text>
                    )}
                </View>
            </View>
        );
    }

    // Simple view for install operation
    if (operation === 'install_from_github' && parsedResult) {
        const { success, skillId } = parsedResult;
        return (
            <View style={styles.container}>
                <View style={styles.header}>
                    <Ionicons
                        name={success ? "checkmark-circle" : "alert-circle"}
                        size={18}
                        color={success ? "#34C759" : "#FF3B30"}
                    />
                    <Text style={styles.headerText}>
                        {success ? `${t('skills.addSkill')} ${t('common.success')}` : `${t('skills.addSkill')} ${t('common.error')}`}
                    </Text>
                </View>
                {skillId && (
                    <View style={styles.content}>
                        <Text style={styles.skillName}>{skillId}</Text>
                    </View>
                )}
            </View>
        );
    }

    // Default fallback
    return (
        <View style={styles.container}>
            <Text style={styles.defaultText} numberOfLines={10}>
                {typeof tool.result === 'string' ? tool.result : JSON.stringify(tool.result, null, 2)}
            </Text>
        </View>
    );
};

const styles = StyleSheet.create((theme) => ({
    container: {
        paddingHorizontal: 12,
        paddingVertical: 8,
    },
    header: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 8,
        marginBottom: 8,
    },
    headerText: {
        fontSize: 14,
        fontWeight: '600',
        color: theme.colors.text,
    },
    content: {
        gap: 6,
    },
    skillName: {
        fontSize: 14,
        fontWeight: '500',
        color: theme.colors.text,
    },
    version: {
        fontSize: 12,
        color: theme.colors.textSecondary,
    },
    description: {
        fontSize: 13,
        color: theme.colors.textSecondary,
        lineHeight: 18,
    },
    defaultText: {
        fontSize: 12,
        color: theme.colors.text,
        fontFamily: 'monospace',
    },
}));
