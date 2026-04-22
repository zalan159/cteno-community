import * as React from 'react';
import { View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { Ionicons } from '@expo/vector-icons';
import type { ToolViewProps } from './_all';
import { Text } from '@/components/StyledText';

export const ActivateSkillView = ({ tool }: ToolViewProps) => {
    // Parse the result to extract skill info
    let skillName = 'Unknown Skill';
    let skillDescription = '';

    console.log('[ActivateSkillView] tool.result type:', typeof tool.result);
    console.log('[ActivateSkillView] tool.result:', tool.result);

    const resultStr = typeof tool.result === 'string' ? tool.result : String(tool.result || '');

    if (resultStr) {
        // Extract skill name from <activated_skill> block
        const nameMatch = resultStr.match(/<name>(.*?)<\/name>/);
        if (nameMatch) {
            skillName = nameMatch[1];
        }

        // Extract description from <description> block
        const descMatch = resultStr.match(/<description>(.*?)<\/description>/);
        if (descMatch) {
            skillDescription = descMatch[1];
        }
    }

    return (
        <View style={styles.container}>
            <View style={styles.content}>
                <View style={styles.iconContainer}>
                    <Ionicons name="checkmark-circle" size={20} color="#34C759" />
                </View>
                <View style={styles.textContainer}>
                    <Text style={styles.skillName}>{skillName}</Text>
                    {skillDescription ? (
                        <Text style={styles.skillDescription} numberOfLines={2}>
                            {skillDescription}
                        </Text>
                    ) : null}
                </View>
            </View>
        </View>
    );
};

const styles = StyleSheet.create((theme) => ({
    container: {
        paddingHorizontal: 12,
        paddingVertical: 8,
    },
    content: {
        flexDirection: 'row',
        alignItems: 'flex-start',
        gap: 10,
    },
    iconContainer: {
        paddingTop: 2,
    },
    textContainer: {
        flex: 1,
        gap: 4,
    },
    skillName: {
        fontSize: 14,
        fontWeight: '500',
        color: theme.colors.text,
    },
    skillDescription: {
        fontSize: 13,
        color: theme.colors.textSecondary,
        lineHeight: 18,
    },
}));
