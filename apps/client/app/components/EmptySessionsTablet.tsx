import React from 'react';
import { View, Pressable } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { Typography } from '@/constants/Typography';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useAllMachines, useLocalSetting } from '@/sync/storage';
import { isMachineOnline } from '@/utils/machineUtils';
import { useRouter } from 'expo-router';
import { Text } from '@/components/StyledText';
import { Modal } from '@/modal';
import { VendorSelector } from '@/components/VendorSelector';
import { quickCreateSession } from '@/utils/quickCreateSession';
import { t } from '@/text';
import type { VendorName } from '@/sync/ops';

const stylesheet = StyleSheet.create((theme) => ({
    container: {
        flex: 1,
        justifyContent: 'center',
        alignItems: 'center',
        paddingHorizontal: 48,
    },
    iconContainer: {
        marginBottom: 24,
    },
    titleText: {
        fontSize: 20,
        color: theme.colors.textSecondary,
        textAlign: 'center',
        marginBottom: 8,
        ...Typography.default('regular'),
    },
    descriptionText: {
        fontSize: 16,
        color: theme.colors.textSecondary,
        textAlign: 'center',
        marginBottom: 24,
        ...Typography.default(),
    },
    button: {
        backgroundColor: theme.colors.button.primary.background,
        paddingHorizontal: 24,
        paddingVertical: 12,
        borderRadius: 12,
        flexDirection: 'row',
        alignItems: 'center',
    },
    buttonDisabled: {
        backgroundColor: theme.colors.textSecondary,
        opacity: 0.6,
    },
    buttonIcon: {
        marginRight: 8,
    },
    buttonText: {
        fontSize: 16,
        color: theme.colors.button.primary.tint,
        fontWeight: '600',
        ...Typography.default('semiBold'),
    },
}));

export function EmptySessionsTablet() {
    const { theme } = useUnistyles();
    const styles = stylesheet;
    const router = useRouter();
    const machines = useAllMachines();
    
    const hasOnlineMachines = React.useMemo(() => {
        return machines.some(machine => isMachineOnline(machine));
    }, [machines]);
    
    const selectedMachineIdFilter = useLocalSetting('selectedMachineIdFilter');
    const [creating, setCreating] = React.useState(false);

    const handleStartNewSession = React.useCallback(() => {
        let modalId: string;
        modalId = Modal.show({
            component: VendorSelector as any,
            props: {
                value: null,
                onChange: async (vendor: VendorName) => {
                    Modal.hide(modalId);
                    setCreating(true);
                    const result = await quickCreateSession({
                        vendor,
                        machineId: selectedMachineIdFilter,
                    });
                    setCreating(false);
                    if (result.ok) {
                        router.push(`/session/${result.sessionId}`);
                    } else {
                        Modal.alert(t('common.error'), result.error);
                    }
                },
                title: t('newSession.selectAgent'),
                machineId: selectedMachineIdFilter,
            },
        });
    }, [selectedMachineIdFilter, router]);
    
    return (
        <View style={styles.container}>
            <Ionicons 
                name="terminal-outline" 
                size={64} 
                color={theme.colors.textSecondary}
                style={styles.iconContainer}
            />
            
            <Text style={styles.titleText}>
                No active sessions
            </Text>
            
            {hasOnlineMachines ? (
                <>
                    <Text style={styles.descriptionText}>
                        Start a new session on any of your connected machines.
                    </Text>
                    <Pressable
                        style={[styles.button, creating && styles.buttonDisabled]}
                        onPress={handleStartNewSession}
                        disabled={creating}
                    >
                        <Ionicons
                            name="add"
                            size={20}
                            color={theme.colors.button.primary.tint}
                            style={styles.buttonIcon}
                        />
                        <Text style={styles.buttonText}>
                            Start New Session
                        </Text>
                    </Pressable>
                </>
            ) : (
                <Text style={styles.descriptionText}>
                    Open a new terminal on your computer to start session.
                </Text>
            )}
        </View>
    );
}
