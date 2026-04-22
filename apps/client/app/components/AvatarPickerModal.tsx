import React from 'react';
import { View, Pressable, Modal } from 'react-native';
import { Image } from 'expo-image';
import { Ionicons } from '@expo/vector-icons';
import { useUnistyles } from 'react-native-unistyles';
import { Typography } from '@/constants/Typography';
import { Text } from '@/components/StyledText';
import { t } from '@/text';
import { MODEL_AVATAR_IMAGES, isModelAvatar } from '@/utils/modelAvatars';

const ICON_AVATAR_OPTIONS = [
    { id: 'default', icon: 'person-circle-outline' },
    { id: 'robot', icon: 'hardware-chip-outline' },
    { id: 'brain', icon: 'bulb-outline' },
    { id: 'star', icon: 'star-outline' },
    { id: 'rocket', icon: 'rocket-outline' },
    { id: 'code', icon: 'code-slash-outline' },
    { id: 'book', icon: 'book-outline' },
    { id: 'search', icon: 'search-outline' },
];

interface AvatarPickerModalProps {
    visible: boolean;
    currentAvatarId: string;
    onSelect: (avatarId: string) => void;
    onClose: () => void;
}

export const AvatarPickerModal: React.FC<AvatarPickerModalProps> = ({
    visible,
    currentAvatarId,
    onSelect,
    onClose,
}) => {
    const { theme } = useUnistyles();

    const handleSelect = (id: string) => {
        onSelect(id);
        onClose();
    };

    return (
        <Modal visible={visible} transparent animationType="fade" onRequestClose={onClose}>
            <Pressable
                style={{ flex: 1, backgroundColor: 'rgba(0,0,0,0.5)', justifyContent: 'center', alignItems: 'center' }}
                onPress={onClose}
            >
                <Pressable
                    style={{
                        backgroundColor: theme.colors.surface,
                        borderRadius: 16,
                        padding: 20,
                        width: 300,
                    }}
                    onPress={(e) => e.stopPropagation()}
                >
                    <Text style={{
                        fontSize: 16,
                        color: theme.colors.text,
                        marginBottom: 16,
                        textAlign: 'center',
                        ...Typography.default('semiBold'),
                    }}>
                        {t('persona.avatar')}
                    </Text>
                    <View style={{ flexDirection: 'row', flexWrap: 'wrap', gap: 8, justifyContent: 'center' }}>
                        {/* Model PNG avatars */}
                        {Object.entries(MODEL_AVATAR_IMAGES).map(([id, uri]) => (
                            <Pressable
                                key={id}
                                onPress={() => handleSelect(id)}
                                style={{
                                    width: 48,
                                    height: 48,
                                    borderRadius: 24,
                                    overflow: 'hidden',
                                    borderWidth: 2,
                                    borderColor: currentAvatarId === id
                                        ? theme.colors.button.primary.background
                                        : 'transparent',
                                }}
                            >
                                <Image
                                    source={{ uri }}
                                    style={{ width: 44, height: 44, borderRadius: 22 }}
                                    contentFit="cover"
                                />
                            </Pressable>
                        ))}
                        {/* Icon avatars */}
                        {ICON_AVATAR_OPTIONS.map((opt) => (
                            <Pressable
                                key={opt.id}
                                onPress={() => handleSelect(opt.id)}
                                style={{
                                    width: 48,
                                    height: 48,
                                    borderRadius: 24,
                                    backgroundColor: currentAvatarId === opt.id
                                        ? theme.colors.button.primary.background
                                        : theme.colors.surfaceHigh,
                                    alignItems: 'center',
                                    justifyContent: 'center',
                                }}
                            >
                                <Ionicons
                                    name={opt.icon as any}
                                    size={24}
                                    color={currentAvatarId === opt.id
                                        ? theme.colors.button.primary.tint
                                        : theme.colors.text}
                                />
                            </Pressable>
                        ))}
                    </View>
                </Pressable>
            </Pressable>
        </Modal>
    );
};
