import * as React from 'react';
import { View, Modal, TouchableOpacity } from 'react-native';
import { CameraView, useCameraPermissions } from 'expo-camera';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { StyleSheet } from 'react-native-unistyles';
import { Text } from '@/components/StyledText';
import { t } from '@/text';

interface QRScannerModalProps {
    visible: boolean;
    onScanned: (data: string) => void;
    onClose: () => void;
}

export function QRScannerModal({ visible, onScanned, onClose }: QRScannerModalProps) {
    const insets = useSafeAreaInsets();
    const [scanned, setScanned] = React.useState(false);
    const [permission, requestPermission] = useCameraPermissions();

    React.useEffect(() => {
        if (visible) {
            setScanned(false);
            if (!permission?.granted) {
                requestPermission();
            }
        }
    }, [visible]);

    const hasPermission = permission?.granted;

    return (
        <Modal visible={visible} animationType="slide" onRequestClose={onClose}>
            <View style={styles.container}>
                {hasPermission ? (
                    <CameraView
                        style={styles.camera}
                        facing="back"
                        barcodeScannerSettings={{ barcodeTypes: ['qr'] }}
                        onBarcodeScanned={scanned ? undefined : (result) => {
                            setScanned(true);
                            onScanned(result.data);
                        }}
                    />
                ) : (
                    <View style={styles.permissionContainer}>
                        <Text style={styles.permissionText}>
                            {t('modals.cameraPermissionsRequiredToConnectTerminal')}
                        </Text>
                        <TouchableOpacity onPress={requestPermission} style={styles.permissionButton}>
                            <Text style={styles.permissionButtonText}>{t('common.ok')}</Text>
                        </TouchableOpacity>
                    </View>
                )}
                <View style={[styles.overlay, { paddingTop: insets.top + 12 }]}>
                    <TouchableOpacity onPress={onClose} style={styles.closeButton}>
                        <Text style={styles.closeText}>{t('common.cancel')}</Text>
                    </TouchableOpacity>
                </View>
            </View>
        </Modal>
    );
}

const styles = StyleSheet.create((theme) => ({
    container: {
        flex: 1,
        backgroundColor: '#000',
    },
    camera: {
        flex: 1,
    },
    overlay: {
        position: 'absolute',
        top: 0,
        left: 0,
        right: 0,
        paddingHorizontal: 16,
    },
    closeButton: {
        alignSelf: 'flex-start',
        padding: 8,
    },
    closeText: {
        color: '#fff',
        fontSize: 17,
    },
    permissionContainer: {
        flex: 1,
        justifyContent: 'center',
        alignItems: 'center',
        padding: 24,
    },
    permissionText: {
        color: '#fff',
        fontSize: 16,
        textAlign: 'center',
        marginBottom: 16,
    },
    permissionButton: {
        backgroundColor: '#007AFF',
        paddingHorizontal: 24,
        paddingVertical: 12,
        borderRadius: 8,
    },
    permissionButtonText: {
        color: '#fff',
        fontSize: 16,
    },
}));
