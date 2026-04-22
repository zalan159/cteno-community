import React from 'react';
import { View, Platform } from 'react-native';
import { Typography } from '@/constants/Typography';
import { RoundButton } from '@/components/RoundButton';
import { useConnectTerminal } from '@/hooks/useConnectTerminal';
import { QRScannerModal } from '@/components/QRScannerModal';
import { t } from '@/text';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Text } from '@/components/StyledText';

const stylesheet = StyleSheet.create((theme) => ({
    container: {
        flex: 1,
        alignItems: 'center',
        justifyContent: 'center',
        marginBottom: 32,
    },
    title: {
        marginBottom: 16,
        textAlign: 'center',
        fontSize: 24,
        color: theme.colors.text,
        ...Typography.default('semiBold'),
    },
    terminalBlock: {
        backgroundColor: theme.colors.surfaceHighest,
        borderRadius: 8,
        padding: 20,
        marginHorizontal: 24,
        marginBottom: 20,
        borderWidth: 1,
        borderColor: theme.colors.divider,
    },
    terminalText: {
        ...Typography.mono(),
        fontSize: 16,
        color: theme.colors.status.connected,
    },
    terminalTextFirst: {
        marginBottom: 8,
    },
    stepsContainer: {
        marginTop: 12,
        marginHorizontal: 24,
        marginBottom: 48,
        width: 250,
    },
    stepRow: {
        flexDirection: 'row',
        alignItems: 'center',
        marginBottom: 8,
    },
    stepRowLast: {
        flexDirection: 'row',
        alignItems: 'center',
    },
    stepNumber: {
        width: 24,
        height: 24,
        borderRadius: 12,
        backgroundColor: theme.colors.surfaceHigh,
        alignItems: 'center',
        justifyContent: 'center',
        marginRight: 12,
    },
    stepNumberText: {
        ...Typography.default('semiBold'),
        fontSize: 14,
        color: theme.colors.text,
    },
    stepText: {
        ...Typography.default(),
        fontSize: 18,
        color: theme.colors.textSecondary,
    },
    buttonsContainer: {
        alignItems: 'center',
        width: '100%',
    },
    buttonWrapper: {
        width: 240,
        marginBottom: 12,
    },
    buttonWrapperSecondary: {
        width: 240,
    },
}));

export function EmptyMainScreen() {
    const { connectTerminal, isLoading, showFallbackScanner, handleFallbackScanned, closeFallbackScanner } = useConnectTerminal();
    const { theme } = useUnistyles();
    const styles = stylesheet;

    return (
        <View style={styles.container}>
            <QRScannerModal visible={showFallbackScanner} onScanned={handleFallbackScanned} onClose={closeFallbackScanner} />
            <Text style={styles.title}>{t('components.emptyMainScreen.readyToCode')}</Text>

            {Platform.OS !== 'web' && (
                <View style={styles.buttonsContainer}>
                    <View style={styles.buttonWrapper}>
                        <RoundButton
                            title={t('settingsAccount.scanToAddSecret')}
                            size="large"
                            loading={isLoading}
                            onPress={connectTerminal}
                        />
                    </View>
                </View>
            )}
        </View>
    );
}
