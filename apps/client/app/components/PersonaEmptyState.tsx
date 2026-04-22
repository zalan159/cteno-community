import React from 'react';
import { View } from 'react-native';
import { Typography } from '@/constants/Typography';
import { Avatar } from '@/components/Avatar';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { t } from '@/text';
import { Text } from '@/components/StyledText';

const stylesheet = StyleSheet.create((theme) => ({
    container: {
        flex: 1,
        justifyContent: 'center',
        alignItems: 'center',
        paddingHorizontal: 48,
    },
    avatarContainer: {
        marginBottom: 16,
    },
    nameText: {
        fontSize: 20,
        color: theme.colors.text,
        textAlign: 'center',
        marginBottom: 4,
        ...Typography.default('semiBold'),
    },
    descriptionText: {
        fontSize: 14,
        color: theme.colors.textSecondary,
        textAlign: 'center',
        marginBottom: 32,
        ...Typography.default('regular'),
    },
    hintText: {
        fontSize: 15,
        color: theme.colors.textSecondary,
        textAlign: 'center',
        lineHeight: 22,
        ...Typography.default(),
    },
}));

interface PersonaEmptyStateProps {
    name: string;
    description: string;
    avatarId: string;
}

export function PersonaEmptyState({ name, description, avatarId }: PersonaEmptyStateProps) {
    const styles = stylesheet;

    return (
        <View style={styles.container}>
            <View style={styles.avatarContainer}>
                <Avatar id={avatarId} size={64} />
            </View>

            <Text style={styles.nameText}>{name}</Text>

            {description ? (
                <Text style={styles.descriptionText}>{description}</Text>
            ) : (
                <View style={{ marginBottom: 32 }} />
            )}

            <Text style={styles.hintText}>
                {t('persona.welcomeHint')}
            </Text>
        </View>
    );
}
