import * as React from 'react';
import { View, ScrollView } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { Ionicons } from '@expo/vector-icons';
import type { ToolViewProps } from './_all';
import { Text } from '@/components/StyledText';
import { t } from '@/text';

export const ListSkillsView = ({ tool }: ToolViewProps) => {
    // Parse the result - handle both string and already-parsed data
    let skills: any[] = [];

    console.log('[ListSkillsView] tool.result type:', typeof tool.result);
    console.log('[ListSkillsView] tool.result:', tool.result);

    if (Array.isArray(tool.result)) {
        // Already parsed as array
        skills = tool.result;
    } else if (tool.result && typeof tool.result === 'string') {
        // String, need to parse
        try {
            const parsed = JSON.parse(tool.result);
            if (Array.isArray(parsed)) {
                skills = parsed;
            }
        } catch (e) {
            console.error('Failed to parse list_skills result:', e);
        }
    } else if (tool.result && typeof tool.result === 'object') {
        // Maybe wrapped in {data: [...]}
        if (Array.isArray(tool.result.data)) {
            skills = tool.result.data;
        }
    }

    console.log('[ListSkillsView] parsed skills count:', skills.length);

    if (skills.length === 0) {
        return (
            <View style={styles.emptyState}>
                <Ionicons name="folder-open-outline" size={24} color="#8E8E93" />
                <Text style={styles.emptyText}>{t('skills.noSkills')}</Text>
            </View>
        );
    }

    return (
        <View style={styles.container}>
            <View style={styles.header}>
                <Ionicons name="list-outline" size={18} color="#007AFF" />
                <Text style={styles.headerText}>{`${t('skills.allSkills')} (${skills.length})`}</Text>
            </View>
            <ScrollView style={styles.scrollView} showsVerticalScrollIndicator={false}>
                {skills.map((skill, index) => (
                    <View key={index} style={styles.skillItem}>
                        <Text style={styles.skillName} numberOfLines={1}>
                            {skill.name || skill.id}
                        </Text>
                        {skill.description && (
                            <Text style={styles.skillDescription} numberOfLines={2}>
                                {skill.description}
                            </Text>
                        )}
                        <Text style={styles.skillRuntime}>{skill.runtime || 'unknown'}</Text>
                    </View>
                ))}
            </ScrollView>
        </View>
    );
};

const styles = StyleSheet.create((theme) => ({
    container: {
        paddingHorizontal: 12,
        paddingVertical: 8,
        maxHeight: 400,
    },
    header: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 8,
        marginBottom: 12,
    },
    headerText: {
        fontSize: 14,
        fontWeight: '600',
        color: theme.colors.text,
    },
    scrollView: {
        flexGrow: 0,
    },
    skillItem: {
        paddingVertical: 8,
        paddingHorizontal: 12,
        backgroundColor: theme.colors.surfaceHigh,
        borderRadius: 6,
        marginBottom: 6,
    },
    skillName: {
        fontSize: 13,
        fontWeight: '500',
        color: theme.colors.text,
        marginBottom: 4,
    },
    skillDescription: {
        fontSize: 12,
        color: theme.colors.textSecondary,
        lineHeight: 16,
        marginBottom: 4,
    },
    skillRuntime: {
        fontSize: 11,
        color: theme.colors.textSecondary,
        textTransform: 'uppercase',
    },
    emptyState: {
        alignItems: 'center',
        justifyContent: 'center',
        paddingVertical: 32,
        gap: 8,
    },
    emptyText: {
        fontSize: 14,
        color: theme.colors.textSecondary,
    },
}));
