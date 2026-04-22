import React from 'react';
import { View, Modal, Pressable, ActivityIndicator } from 'react-native';
import { useUnistyles } from 'react-native-unistyles';
import { Typography } from '@/constants/Typography';
import { Text } from '@/components/StyledText';
import { Ionicons } from '@expo/vector-icons';
import { t } from '@/text';

interface DesktopUpdateModalProps {
    visible: boolean;
    onClose: () => void;
    version?: string;
    notes?: string;
    downloading: boolean;
    progress?: number;
    error?: string | null;
    onConfirm: () => void;
}

export const DesktopUpdateModal = React.memo(({
    visible, onClose, version, notes, downloading, progress, error, onConfirm,
}: DesktopUpdateModalProps) => {
    const { theme } = useUnistyles();

    const isComplete = !downloading && progress === 100;
    const accentColor = theme.colors.success;

    return (
        <Modal
            visible={visible}
            transparent
            animationType="fade"
            onRequestClose={onClose}
        >
            <Pressable
                style={{
                    flex: 1,
                    backgroundColor: 'rgba(0,0,0,0.4)',
                    justifyContent: 'center',
                    alignItems: 'center',
                }}
                onPress={downloading ? undefined : onClose}
            >
                <Pressable
                    style={{
                        backgroundColor: theme.colors.groupped.background,
                        borderRadius: 16,
                        padding: 24,
                        width: 340,
                        maxWidth: '90%',
                    }}
                    onPress={() => {}} // prevent close on inner press
                >
                    {/* Icon */}
                    <View style={{ alignItems: 'center', marginBottom: 16 }}>
                        <View style={{
                            width: 56,
                            height: 56,
                            borderRadius: 28,
                            backgroundColor: downloading ? theme.colors.groupped.background : `${accentColor}1A`,
                            alignItems: 'center',
                            justifyContent: 'center',
                        }}>
                            {downloading ? (
                                <ActivityIndicator size="small" color={accentColor} />
                            ) : isComplete ? (
                                <Ionicons name="checkmark-circle" size={32} color={accentColor} />
                            ) : (
                                <Ionicons name="arrow-up-circle" size={32} color={accentColor} />
                            )}
                        </View>
                    </View>

                    {/* Title */}
                    <Text style={{
                        textAlign: 'center',
                        fontSize: 18,
                        marginBottom: 8,
                        color: theme.colors.text,
                        ...Typography.default('semiBold'),
                    }}>
                        {downloading
                            ? t('updateBanner.downloadingGeneric')
                            : isComplete
                                ? t('updateBanner.readyToRestart')
                                : t('updateBanner.desktopUpdateAvailable')}
                    </Text>

                    {/* Version */}
                    {version && !downloading && !isComplete && (
                        <Text style={{
                            textAlign: 'center',
                            fontSize: 14,
                            color: theme.colors.textSecondary,
                            marginBottom: 8,
                        }}>
                            v{version}
                        </Text>
                    )}

                    {/* Notes */}
                    {notes && !downloading && !isComplete && (
                        <Text style={{
                            textAlign: 'center',
                            fontSize: 13,
                            color: theme.colors.textSecondary,
                            marginBottom: 16,
                            lineHeight: 18,
                        }}>
                            {notes}
                        </Text>
                    )}

                    {/* Error */}
                    {error && !downloading && (
                        <Text style={{
                            textAlign: 'center',
                            fontSize: 13,
                            color: theme.colors.textDestructive,
                            marginBottom: 16,
                            lineHeight: 18,
                        }}>
                            {error}
                        </Text>
                    )}

                    {/* Progress bar */}
                    {downloading && (
                        <View style={{ marginBottom: 16 }}>
                            <View style={{
                                height: 6,
                                backgroundColor: theme.colors.groupped.background,
                                borderRadius: 3,
                                overflow: 'hidden',
                            }}>
                                <View style={{
                                    height: '100%',
                                    width: `${progress ?? 0}%`,
                                    backgroundColor: accentColor,
                                    borderRadius: 3,
                                }} />
                            </View>
                            {progress != null && (
                                <Text style={{
                                    textAlign: 'center',
                                    fontSize: 12,
                                    color: theme.colors.textSecondary,
                                    marginTop: 6,
                                }}>
                                    {progress}%
                                </Text>
                            )}
                        </View>
                    )}

                    {/* Buttons */}
                    {!downloading && (
                        <View style={{ gap: 8, marginTop: 8 }}>
                            <Pressable
                                style={{
                                    backgroundColor: accentColor,
                                    borderRadius: 10,
                                    paddingVertical: 12,
                                    alignItems: 'center',
                                }}
                                onPress={onConfirm}
                            >
                                <Text style={{
                                    color: '#FFFFFF',
                                    fontSize: 16,
                                    ...Typography.default('semiBold'),
                                }}>
                                    {isComplete
                                        ? t('updateBanner.restartNow')
                                        : t('updateBanner.confirmUpdate')}
                                </Text>
                            </Pressable>
                            {!isComplete && (
                                <Pressable
                                    style={{
                                        borderRadius: 10,
                                        paddingVertical: 12,
                                        alignItems: 'center',
                                    }}
                                    onPress={onClose}
                                >
                                    <Text style={{
                                        color: theme.colors.textSecondary,
                                        fontSize: 16,
                                    }}>
                                        {t('common.cancel')}
                                    </Text>
                                </Pressable>
                            )}
                        </View>
                    )}
                </Pressable>
            </Pressable>
        </Modal>
    );
});
