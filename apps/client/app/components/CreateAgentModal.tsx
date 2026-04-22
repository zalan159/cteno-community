import React, { useState } from 'react';
import { View, ScrollView, Pressable, Modal, TextInput } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useUnistyles } from 'react-native-unistyles';
import { Typography } from '@/constants/Typography';
import { Text } from '@/components/StyledText';
import { t } from '@/text';

interface CreateAgentModalProps {
    visible: boolean;
    onClose: () => void;
    onCreate: (params: {
        id: string;
        name: string;
        description: string;
        model?: string;
        scope: 'global' | 'workspace';
    }) => Promise<void>;
}

export const CreateAgentModal: React.FC<CreateAgentModalProps> = ({
    visible,
    onClose,
    onCreate,
}) => {
    const { theme } = useUnistyles();
    const [id, setId] = useState('');
    const [name, setName] = useState('');
    const [description, setDescription] = useState('');
    const [model, setModel] = useState('');
    const [scope, setScope] = useState<'global' | 'workspace'>('workspace');
    const [saving, setSaving] = useState(false);

    // Auto-generate ID from name
    const handleNameChange = (text: string) => {
        setName(text);
        // Only auto-set ID if user hasn't manually edited it
        if (!id || id === slugify(name)) {
            setId(slugify(text));
        }
    };

    const handleCreate = async () => {
        if (!id.trim() || !name.trim()) return;
        try {
            setSaving(true);
            await onCreate({
                id: id.trim(),
                name: name.trim(),
                description: description.trim(),
                model: model.trim() || undefined,
                scope,
            });
            // Reset form
            setId('');
            setName('');
            setDescription('');
            setModel('');
            setScope('workspace');
            onClose();
        } catch (err) {
            console.error('Failed to create agent:', err);
        } finally {
            setSaving(false);
        }
    };

    const canCreate = id.trim() && name.trim() && !saving;

    return (
        <Modal
            visible={visible}
            transparent
            animationType="slide"
            onRequestClose={onClose}
        >
            <View
                style={{
                    flex: 1,
                    backgroundColor: 'rgba(0,0,0,0.5)',
                    justifyContent: 'flex-end',
                }}
            >
                <View
                    style={{
                        backgroundColor: theme.colors.surface,
                        borderTopLeftRadius: 20,
                        borderTopRightRadius: 20,
                        maxHeight: '80%',
                    }}
                >
                    {/* Header */}
                    <View
                        style={{
                            flexDirection: 'row',
                            justifyContent: 'space-between',
                            alignItems: 'center',
                            padding: 16,
                            borderBottomWidth: 1,
                            borderBottomColor: theme.colors.divider,
                        }}
                    >
                        <Pressable onPress={onClose}>
                            <Text
                                style={{
                                    fontSize: 16,
                                    color: theme.colors.textSecondary,
                                    ...Typography.default(),
                                }}
                            >
                                {t('common.cancel')}
                            </Text>
                        </Pressable>
                        <Text
                            style={{
                                fontSize: 17,
                                color: theme.colors.text,
                                ...Typography.default('semiBold'),
                            }}
                        >
                            {t('agent.newAgent')}
                        </Text>
                        <Pressable
                            onPress={handleCreate}
                            disabled={!canCreate}
                        >
                            <Text
                                style={{
                                    fontSize: 16,
                                    color: canCreate
                                        ? theme.colors.textLink
                                        : theme.colors.textSecondary,
                                    ...Typography.default('semiBold'),
                                }}
                            >
                                {saving ? t('agent.creating') : t('common.create')}
                            </Text>
                        </Pressable>
                    </View>

                    <ScrollView style={{ padding: 16 }}>
                        {/* Name input */}
                        <Text
                            style={{
                                fontSize: 13,
                                color: theme.colors.textSecondary,
                                marginBottom: 8,
                                ...Typography.default('semiBold'),
                            }}
                        >
                            {t('agent.name')}
                        </Text>
                        <TextInput
                            value={name}
                            onChangeText={handleNameChange}
                            placeholder={t('agent.namePlaceholder')}
                            placeholderTextColor={theme.colors.textSecondary}
                            style={{
                                backgroundColor: theme.colors.surfaceHigh,
                                borderRadius: 8,
                                padding: 12,
                                fontSize: 16,
                                color: theme.colors.text,
                                marginBottom: 16,
                                ...Typography.default(),
                            }}
                        />

                        {/* ID input */}
                        <Text
                            style={{
                                fontSize: 13,
                                color: theme.colors.textSecondary,
                                marginBottom: 8,
                                ...Typography.default('semiBold'),
                            }}
                        >
                            {t('agent.id')}
                        </Text>
                        <TextInput
                            value={id}
                            onChangeText={setId}
                            placeholder={t('agent.idPlaceholder')}
                            placeholderTextColor={theme.colors.textSecondary}
                            autoCapitalize="none"
                            autoCorrect={false}
                            style={{
                                backgroundColor: theme.colors.surfaceHigh,
                                borderRadius: 8,
                                padding: 12,
                                fontSize: 16,
                                color: theme.colors.text,
                                marginBottom: 4,
                                ...Typography.default(),
                            }}
                        />
                        <Text
                            style={{
                                fontSize: 12,
                                color: theme.colors.textSecondary,
                                marginBottom: 16,
                                ...Typography.default(),
                            }}
                        >
                            {t('agent.idHint')}
                        </Text>

                        {/* Description input */}
                        <Text
                            style={{
                                fontSize: 13,
                                color: theme.colors.textSecondary,
                                marginBottom: 8,
                                ...Typography.default('semiBold'),
                            }}
                        >
                            {t('agent.description')}
                        </Text>
                        <TextInput
                            value={description}
                            onChangeText={setDescription}
                            placeholder={t('agent.descriptionPlaceholder')}
                            placeholderTextColor={theme.colors.textSecondary}
                            multiline
                            numberOfLines={3}
                            style={{
                                backgroundColor: theme.colors.surfaceHigh,
                                borderRadius: 8,
                                padding: 12,
                                fontSize: 16,
                                color: theme.colors.text,
                                marginBottom: 16,
                                minHeight: 80,
                                textAlignVertical: 'top',
                                ...Typography.default(),
                            }}
                        />

                        {/* Model input (optional) */}
                        <Text
                            style={{
                                fontSize: 13,
                                color: theme.colors.textSecondary,
                                marginBottom: 8,
                                ...Typography.default('semiBold'),
                            }}
                        >
                            {t('agent.model')}
                        </Text>
                        <TextInput
                            value={model}
                            onChangeText={setModel}
                            placeholder={t('agent.modelPlaceholder')}
                            placeholderTextColor={theme.colors.textSecondary}
                            autoCapitalize="none"
                            autoCorrect={false}
                            style={{
                                backgroundColor: theme.colors.surfaceHigh,
                                borderRadius: 8,
                                padding: 12,
                                fontSize: 16,
                                color: theme.colors.text,
                                marginBottom: 16,
                                ...Typography.default(),
                            }}
                        />

                        {/* Scope toggle */}
                        <Text
                            style={{
                                fontSize: 13,
                                color: theme.colors.textSecondary,
                                marginBottom: 8,
                                ...Typography.default('semiBold'),
                            }}
                        >
                            {t('agent.scope')}
                        </Text>
                        <View style={{ flexDirection: 'row', gap: 8, marginBottom: 32 }}>
                            <Pressable
                                onPress={() => setScope('workspace')}
                                style={{
                                    flex: 1,
                                    flexDirection: 'row',
                                    alignItems: 'center',
                                    justifyContent: 'center',
                                    paddingVertical: 10,
                                    borderRadius: 8,
                                    borderWidth: 2,
                                    borderColor: scope === 'workspace'
                                        ? theme.colors.button.primary.background
                                        : 'transparent',
                                    backgroundColor: theme.colors.surfaceHigh,
                                }}
                            >
                                <Ionicons
                                    name="folder-outline"
                                    size={16}
                                    color={scope === 'workspace' ? theme.colors.button.primary.background : theme.colors.text}
                                />
                                <Text
                                    style={{
                                        fontSize: 14,
                                        color: theme.colors.text,
                                        marginLeft: 6,
                                        ...Typography.default(scope === 'workspace' ? 'semiBold' : 'regular'),
                                    }}
                                >
                                    {t('agent.scopeWorkspace')}
                                </Text>
                            </Pressable>
                            <Pressable
                                onPress={() => setScope('global')}
                                style={{
                                    flex: 1,
                                    flexDirection: 'row',
                                    alignItems: 'center',
                                    justifyContent: 'center',
                                    paddingVertical: 10,
                                    borderRadius: 8,
                                    borderWidth: 2,
                                    borderColor: scope === 'global'
                                        ? theme.colors.button.primary.background
                                        : 'transparent',
                                    backgroundColor: theme.colors.surfaceHigh,
                                }}
                            >
                                <Ionicons
                                    name="globe-outline"
                                    size={16}
                                    color={scope === 'global' ? theme.colors.button.primary.background : theme.colors.text}
                                />
                                <Text
                                    style={{
                                        fontSize: 14,
                                        color: theme.colors.text,
                                        marginLeft: 6,
                                        ...Typography.default(scope === 'global' ? 'semiBold' : 'regular'),
                                    }}
                                >
                                    {t('agent.scopeGlobal')}
                                </Text>
                            </Pressable>
                        </View>
                    </ScrollView>
                </View>
            </View>
        </Modal>
    );
};

/** Convert a name to a URL-safe slug */
function slugify(text: string): string {
    return text
        .toLowerCase()
        .trim()
        .replace(/[^a-z0-9\u4e00-\u9fa5]+/g, '-')
        .replace(/^-+|-+$/g, '');
}
