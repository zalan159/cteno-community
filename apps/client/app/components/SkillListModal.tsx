import React from 'react';
import { View, Pressable, ScrollView } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useUnistyles } from 'react-native-unistyles';
import { Typography } from '@/constants/Typography';
import { t } from '@/text';
import { Text } from '@/components/StyledText';
import type { SkillListItem } from '@/sync/ops';

interface SkillListModalProps {
    skills: SkillListItem[];
    onSelect?: (skill: SkillListItem) => void;
    onClose: () => void;
}

const SOURCE_ORDER = ['workspace', 'user', 'installed', 'community', 'global', 'builtin'] as const;

const SOURCE_LABELS: Record<string, string> = {
    workspace: 'Workspace',
    user: 'User',
    installed: 'Installed',
    community: 'Community',
    global: 'Global',
    builtin: 'Built-in',
};

const SOURCE_COLORS: Record<string, string> = {
    workspace: '#5856D6',
    user: '#FF9500',
    installed: '#007AFF',
    community: '#34C759',
    global: '#007AFF',
    builtin: '#8E8E93',
};

export function SkillListModal({ skills, onSelect, onClose }: SkillListModalProps) {
    const { theme } = useUnistyles();

    const grouped = React.useMemo(() => {
        const map: Record<string, SkillListItem[]> = {};
        for (const skill of skills) {
            const src = skill.source || 'builtin';
            if (!map[src]) map[src] = [];
            map[src].push(skill);
        }
        return map;
    }, [skills]);

    return (
        <View style={{
            backgroundColor: theme.colors.surface,
            borderRadius: 14,
            width: 340,
            maxHeight: 500,
            overflow: 'hidden',
            shadowColor: theme.colors.shadow.color,
            shadowOffset: { width: 0, height: 2 },
            shadowOpacity: 0.25,
            shadowRadius: 4,
            elevation: 5,
        }}>
            {/* Header */}
            <View style={{
                paddingHorizontal: 20,
                paddingTop: 20,
                paddingBottom: 12,
            }}>
                <Text style={{
                    fontSize: 17,
                    textAlign: 'center',
                    color: theme.colors.text,
                    ...Typography.default('semiBold'),
                }}>
                    {t('skills.title')}
                </Text>
                <Text style={{
                    fontSize: 13,
                    textAlign: 'center',
                    color: theme.colors.textSecondary,
                    marginTop: 4,
                    lineHeight: 18,
                    ...Typography.default(),
                }}>
                    {t('skills.subtitle')}
                </Text>
            </View>

            {/* Skill list */}
            <ScrollView style={{ maxHeight: 360 }}>
                {skills.length === 0 && (
                    <View style={{ paddingHorizontal: 20, paddingVertical: 24, alignItems: 'center' }}>
                        <Ionicons name="extension-puzzle-outline" size={32} color={theme.colors.textSecondary} />
                        <Text style={{
                            fontSize: 14,
                            color: theme.colors.textSecondary,
                            marginTop: 8,
                            textAlign: 'center',
                            ...Typography.default(),
                        }}>
                            {t('skills.noSkills')}
                        </Text>
                    </View>
                )}

                {SOURCE_ORDER.map((source) => {
                    const items = grouped[source];
                    if (!items?.length) return null;
                    return (
                        <View key={source}>
                            {/* Section header */}
                            <View style={{
                                paddingHorizontal: 20,
                                paddingTop: 12,
                                paddingBottom: 4,
                                borderTopWidth: 0.5,
                                borderTopColor: theme.colors.divider,
                            }}>
                                <Text style={{
                                    fontSize: 12,
                                    color: SOURCE_COLORS[source] || theme.colors.textSecondary,
                                    textTransform: 'uppercase',
                                    letterSpacing: 0.5,
                                    ...Typography.default('semiBold'),
                                }}>
                                    {SOURCE_LABELS[source] || source}
                                </Text>
                            </View>
                            {items.map((skill) => (
                                <Pressable
                                    key={skill.id}
                                    onPress={onSelect ? () => { onSelect(skill); onClose(); } : undefined}
                                    style={({ pressed }) => ({
                                        flexDirection: 'row',
                                        alignItems: 'center',
                                        paddingHorizontal: 20,
                                        paddingVertical: 10,
                                        backgroundColor: pressed ? theme.colors.surfaceRipple : 'transparent',
                                    })}
                                >
                                    <Ionicons
                                        name="extension-puzzle-outline"
                                        size={20}
                                        color={SOURCE_COLORS[skill.source || 'builtin']}
                                        style={{ marginRight: 12 }}
                                    />
                                    <View style={{ flex: 1 }}>
                                        <Text style={{
                                            fontSize: 15,
                                            color: theme.colors.text,
                                            ...Typography.default('semiBold'),
                                        }}>
                                            {skill.name}
                                        </Text>
                                        {skill.description ? (
                                            <Text style={{
                                                fontSize: 12,
                                                color: theme.colors.textSecondary,
                                                marginTop: 2,
                                                ...Typography.default(),
                                            }} numberOfLines={2}>
                                                {skill.description}
                                            </Text>
                                        ) : null}
                                    </View>
                                    {skill.version ? (
                                        <Text style={{
                                            fontSize: 11,
                                            color: theme.colors.textSecondary,
                                            marginLeft: 8,
                                            ...Typography.default(),
                                        }}>
                                            v{skill.version}
                                        </Text>
                                    ) : null}
                                </Pressable>
                            ))}
                        </View>
                    );
                })}
            </ScrollView>

            {/* Close button */}
            <Pressable
                onPress={onClose}
                style={({ pressed }) => ({
                    borderTopWidth: 0.5,
                    borderTopColor: theme.colors.divider,
                    paddingVertical: 12,
                    alignItems: 'center',
                    justifyContent: 'center',
                    backgroundColor: pressed ? theme.colors.surfaceRipple : 'transparent',
                })}
            >
                <Text style={{
                    fontSize: 17,
                    color: theme.colors.textLink,
                    ...Typography.default('semiBold'),
                }}>
                    {t('common.ok')}
                </Text>
            </Pressable>
        </View>
    );
}
