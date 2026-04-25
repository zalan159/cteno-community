import { Ionicons, Octicons } from '@expo/vector-icons';
import { Image } from 'expo-image';
import * as ImagePicker from 'expo-image-picker';
import * as ImageManipulator from 'expo-image-manipulator';
import * as FileSystem from 'expo-file-system';
import * as React from 'react';
import { View, Platform, Pressable, ActivityIndicator, ScrollView, Alert } from 'react-native';
import { layout } from './layout';
import { MultiTextInput, KeyPressEvent, type MultiTextInputHandle } from './MultiTextInput';
import { Typography } from '@/constants/Typography';
import { hapticsLight, hapticsError } from './haptics';
import { Modal } from '@/modal';
import { StatusDot } from './StatusDot';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useSetting } from '@/sync/storage';
import { Text } from '@/components/StyledText';
import { QuotaPopover } from './QuotaPopover';
import type { Metadata, VendorQuotaId } from '@/sync/storageTypes';
import { PermissionRequestCard } from './tools/PermissionRequestCard';

export type PickedImage = {
    uri: string;
    media_type: string;
    data: string; // base64
};

export type PendingImage = PickedImage;

interface PersonaChatInputProps {
    value: string;
    placeholder: string;
    onChangeText: (text: string) => void;
    onSend: () => void;
    onMicPress?: () => void;
    isMicActive?: boolean;
    onAbort?: () => void | Promise<void>;
    showAbortButton?: boolean;
    connectionStatus?: {
        text: string;
        color: string;
        dotColor: string;
        isPulsing?: boolean;
        compressionInfo?: { text: string; color: string; percentage: number };
    };
    activeSkillCount?: number;
    onSkillClick?: () => void;
    selectedSkills?: Array<{ id: string; name: string }>;
    onRemoveSkill?: (id: string) => void;
    agentCount?: number;
    onAgentClick?: () => void;
    onClear?: () => void;
    isClearLoading?: boolean;
    supportsVision?: boolean;
    pendingImages?: PendingImage[];
    onAddImage?: (image: PickedImage) => void;
    onRemoveImage?: (index: number) => void;
    modelName?: string;
    onModelPress?: () => void;
    effortName?: string;
    onEffortPress?: () => void;
    autocompleteOptions?: Array<{
        id: string;
        label?: string;
        description?: string;
    }>;
    // Machine-level usage indicator — `vendor` picks which quota set
    // (claude / codex / gemini) to read from the global store. `machineId`
    // is optional; QuotaPopover falls back to the only registered machine.
    quotaVendor?: VendorQuotaId | null;
    quotaMachineId?: string | null;
    quotaPreferredModelId?: string | null;
    sessionId?: string;
    metadata?: Metadata | null;
    pendingPermission?: {
        id: string;
        tool: string;
        arguments: any;
        createdAt?: number | null;
    } | null;
    todoToggle?: {
        completed: number;
        total: number;
        collapsed: boolean;
        onPress: () => void;
    } | null;
}

function extractActiveMention(text: string, cursorPosition: number): { query: string; start: number; end: number } | null {
    if (!text) return null;
    const textBeforeCursor = text.slice(0, cursorPosition);
    const match = textBeforeCursor.match(/@([a-zA-Z0-9_-]*)$/);
    if (!match || match.index === undefined) return null;
    return {
        query: match[1] || '',
        start: match.index,
        end: cursorPosition,
    };
}

const stylesheet = StyleSheet.create((theme) => ({
    container: {
        alignItems: 'center',
        paddingBottom: 8,
        paddingTop: 8,
    },
    innerContainer: {
        width: '100%',
        position: 'relative',
    },
    unifiedPanel: {
        backgroundColor: theme.colors.input.background,
        borderRadius: Platform.select({ default: 16, android: 20 }),
        overflow: 'hidden',
        paddingVertical: 2,
        paddingBottom: 8,
        paddingHorizontal: 8,
    },
    inputContainer: {
        flexDirection: 'row',
        alignItems: 'center',
        borderWidth: 0,
        paddingLeft: 8,
        paddingRight: 8,
        paddingVertical: 4,
        minHeight: 40,
    },
    actionButtonsContainer: {
        flexDirection: 'row',
        alignItems: 'center',
        justifyContent: 'space-between',
        paddingHorizontal: 0,
    },
    statusInfoRow: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 6,
        paddingHorizontal: 8,
        paddingBottom: 6,
    },
    statusRow: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 6,
        flex: 1,
        paddingHorizontal: 8,
    },
    statusText: {
        fontSize: 12,
        color: theme.colors.textSecondary,
        ...Typography.default(),
    },
    todoStatusButton: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 3,
        flexShrink: 0,
    },
    sendButton: {
        width: 32,
        height: 32,
        borderRadius: 16,
        justifyContent: 'center',
        alignItems: 'center',
        flexShrink: 0,
        marginLeft: 8,
    },
    sendButtonActive: {
        backgroundColor: theme.colors.button.primary.background,
    },
    sendButtonInactive: {
        backgroundColor: theme.colors.button.primary.disabled,
    },
    sendButtonIcon: {
        color: theme.colors.button.primary.tint,
    },
}));

export const PersonaChatInput = React.memo(({
    value,
    placeholder,
    onChangeText,
    onSend,
    onMicPress,
    isMicActive,
    onAbort,
    showAbortButton,
    connectionStatus,
    activeSkillCount,
    onSkillClick,
    agentCount,
    onAgentClick,
    onClear,
    isClearLoading,
    supportsVision,
    pendingImages,
    onAddImage,
    onRemoveImage,
    selectedSkills,
    onRemoveSkill,
    modelName,
    onModelPress,
    effortName,
    onEffortPress,
    autocompleteOptions,
    quotaVendor,
    quotaMachineId,
    quotaPreferredModelId,
    sessionId,
    metadata,
    pendingPermission,
    todoToggle,
}: PersonaChatInputProps) => {
    const { theme } = useUnistyles();
    const styles = stylesheet;
    const agentInputEnterToSend = useSetting('agentInputEnterToSend');
    const hasText = value.trim().length > 0;
    const hasImages = (pendingImages?.length ?? 0) > 0;
    const hasSkills = (selectedSkills?.length ?? 0) > 0;
    const hasContent = hasText || hasImages || hasSkills;
    const isPermissionBlocked = !!pendingPermission;
    const canSend = hasContent && !isPermissionBlocked;
    const [toastMessage, setToastMessage] = React.useState<string | null>(null);
    const inputRef = React.useRef<MultiTextInputHandle>(null);
    const [selection, setSelection] = React.useState({ start: value.length, end: value.length });
    const [selectedAutocompleteIndex, setSelectedAutocompleteIndex] = React.useState(0);

    const activeMention = React.useMemo(
        () => extractActiveMention(value, selection.start),
        [value, selection.start],
    );

    const autocompleteSuggestions = React.useMemo(() => {
        if (!autocompleteOptions || autocompleteOptions.length === 0 || !activeMention) {
            return [];
        }
        const query = activeMention.query.trim().toLowerCase();
        const filtered = autocompleteOptions.filter((option) => {
            const id = option.id.toLowerCase();
            const label = (option.label || '').toLowerCase();
            if (!query) return true;
            return id.startsWith(query) || label.startsWith(query) || id.includes(query) || label.includes(query);
        });
        return filtered.slice(0, 6);
    }, [activeMention, autocompleteOptions]);

    React.useEffect(() => {
        setSelectedAutocompleteIndex((current) => {
            if (autocompleteSuggestions.length === 0) return 0;
            return Math.min(current, autocompleteSuggestions.length - 1);
        });
    }, [autocompleteSuggestions]);

    const applyAutocompleteSuggestion = React.useCallback((roleId: string) => {
        if (!activeMention) return;
        const nextText = `${value.slice(0, activeMention.start)}@${roleId} ${value.slice(activeMention.end)}`;
        const nextCursor = activeMention.start + roleId.length + 2;
        inputRef.current?.setTextAndSelection(nextText, { start: nextCursor, end: nextCursor });
        setSelectedAutocompleteIndex(0);
    }, [activeMention, value]);

    const handlePickImage = React.useCallback(async () => {
        if (!onAddImage) return;

        if (Platform.OS === 'web') {
            // On web, use native <input> without accept filter.
            // macOS WebKit ignores accept and silently blocks onchange for non-matching files,
            // so we remove it and validate in onchange instead.
            const input = document.createElement('input');
            input.type = 'file';
            input.multiple = true;
            input.style.display = 'none';
            document.body.appendChild(input);
            input.onchange = async () => {
                document.body.removeChild(input);
                const files = input.files;
                if (!files || files.length === 0) return;
                const MAX_BASE64_SIZE = 5 * 1024 * 1024;
                let rejectedNames: string[] = [];
                for (const file of Array.from(files)) {
                    if (!file.type.startsWith('image/')) {
                        rejectedNames.push(file.name);
                        continue;
                    }
                    if (file.size > 20 * 1024 * 1024) {
                        window.alert('File too large: max image size is 20MB.');
                        continue;
                    }
                    try {
                        // Load image into canvas for resize/compress
                        const blobUrl = URL.createObjectURL(file);
                        const img = await new Promise<HTMLImageElement>((resolve, reject) => {
                            const el = new window.Image();
                            el.onload = () => resolve(el);
                            el.onerror = reject;
                            el.src = blobUrl;
                        });
                        const MAX_DIM = 1536;
                        let w = img.naturalWidth;
                        let h = img.naturalHeight;
                        if (w > MAX_DIM || h > MAX_DIM) {
                            const scale = MAX_DIM / Math.max(w, h);
                            w = Math.round(w * scale);
                            h = Math.round(h * scale);
                        }
                        const canvas = document.createElement('canvas');
                        canvas.width = w;
                        canvas.height = h;
                        canvas.getContext('2d')!.drawImage(img, 0, 0, w, h);
                        const dataUrl = canvas.toDataURL('image/jpeg', 0.8);
                        URL.revokeObjectURL(blobUrl);

                        const base64 = dataUrl.split(',')[1];
                        if (!base64 || base64.length > MAX_BASE64_SIZE) {
                            setToastMessage('Image is too large (max 5MB after compression)');
                            setTimeout(() => setToastMessage(null), 3000);
                            continue;
                        }
                        onAddImage({ uri: dataUrl, media_type: 'image/jpeg', data: base64 });
                    } catch (err) {
                        console.error('Failed to read file:', err);
                    }
                }
                if (rejectedNames.length > 0) {
                    console.warn('[ImagePicker] Rejected non-image files:', rejectedNames);
                    setToastMessage('Only image files are supported (JPEG, PNG, WebP, GIF)');
                    setTimeout(() => setToastMessage(null), 3000);
                }
            };
            input.click();
            return;
        }

        // Native (iOS/Android) path - use expo-image-picker
        try {
            const result = await ImagePicker.launchImageLibraryAsync({
                mediaTypes: ['images'],
                quality: 0.8,
                base64: true,
                allowsMultipleSelection: true,
            });
            if (!result.canceled && result.assets) {
                const MAX_DIMENSION = 1536;
                const MAX_BASE64_SIZE = 5 * 1024 * 1024;

                for (const asset of result.assets) {
                    const w = asset.width || 0;
                    const h = asset.height || 0;
                    const needsResize = w > MAX_DIMENSION || h > MAX_DIMENSION;

                    if (needsResize) {
                        try {
                            const actions: ImageManipulator.Action[] = [
                                { resize: w >= h ? { width: MAX_DIMENSION } : { height: MAX_DIMENSION } },
                            ];
                            const manipulated = await ImageManipulator.manipulateAsync(
                                asset.uri,
                                actions,
                                { compress: 0.8, format: ImageManipulator.SaveFormat.JPEG, base64: true },
                            );
                            if (manipulated.base64 && manipulated.base64.length <= MAX_BASE64_SIZE) {
                                onAddImage({ uri: manipulated.uri, media_type: 'image/jpeg', data: manipulated.base64 });
                                continue;
                            }
                        } catch (compressErr) {
                            console.warn('Image compression failed, using original:', compressErr);
                        }
                    }

                    if (!asset.base64) continue;
                    if (asset.base64.length > MAX_BASE64_SIZE) {
                        console.warn(`Image too large (${(asset.base64.length / 1024 / 1024).toFixed(1)}MB), skipping`);
                        continue;
                    }
                    const ext = (asset.uri.split('.').pop() || 'jpeg').toLowerCase();
                    const mediaType = ext === 'png' ? 'image/png' : ext === 'webp' ? 'image/webp' : ext === 'gif' ? 'image/gif' : 'image/jpeg';
                    onAddImage({ uri: asset.uri, media_type: mediaType, data: asset.base64 });
                }
            }
        } catch (err) {
            console.error('Image picker error:', err);
        }
    }, [onAddImage]);


    // Abort button state (matches AgentInput pattern)
    const [isAborting, setIsAborting] = React.useState(false);

    // Reset isAborting when thinking stops
    React.useEffect(() => {
        if (!showAbortButton && isAborting) {
            setIsAborting(false);
        }
    }, [showAbortButton, isAborting]);

    const handleAbortPress = React.useCallback(async () => {
        if (!onAbort || isAborting) return;
        hapticsError();
        setIsAborting(true);
        try {
            await onAbort();
        } catch (error) {
            console.error('Abort RPC call failed:', error);
            setIsAborting(false);
        }
    }, [onAbort, isAborting]);

    const handleKeyPress = React.useCallback((event: KeyPressEvent): boolean => {
        if (isPermissionBlocked) {
            return true;
        }

        if (autocompleteSuggestions.length > 0) {
            if (event.key === 'ArrowDown') {
                setSelectedAutocompleteIndex((current) => (current + 1) % autocompleteSuggestions.length);
                return true;
            }
            if (event.key === 'ArrowUp') {
                setSelectedAutocompleteIndex((current) => (current - 1 + autocompleteSuggestions.length) % autocompleteSuggestions.length);
                return true;
            }
            if (event.key === 'Tab') {
                const selected = autocompleteSuggestions[selectedAutocompleteIndex];
                if (selected) {
                    applyAutocompleteSuggestion(selected.id);
                    return true;
                }
            }
        }

        // Handle Escape for abort
        if (event.key === 'Escape' && showAbortButton && onAbort && !isAborting) {
            handleAbortPress();
            return true;
        }

        // Handle Enter to send (web only, respects setting)
        if (Platform.OS === 'web') {
            if (agentInputEnterToSend && event.key === 'Enter' && !event.shiftKey) {
                if (canSend) {
                    onSend();
                    return true;
                }
            }
        }
        return false;
    }, [
        agentInputEnterToSend,
        applyAutocompleteSuggestion,
        autocompleteSuggestions,
        canSend,
        handleAbortPress,
        isPermissionBlocked,
        isAborting,
        onAbort,
        onSend,
        selectedAutocompleteIndex,
        showAbortButton,
    ]);

    return (
        <View style={styles.container}>
            {toastMessage && (
                <View style={{
                    alignItems: 'center',
                    paddingVertical: 6,
                }}>
                    <View style={{
                        backgroundColor: theme.colors.warningCritical,
                        paddingHorizontal: 16, paddingVertical: 8,
                        borderRadius: 8,
                    }}>
                        <Text style={{ color: '#fff', fontSize: 13 }}>{toastMessage}</Text>
                    </View>
                </View>
            )}
            <View style={[styles.innerContainer, { maxWidth: layout.maxWidth, paddingHorizontal: 16 }]}>
                {/* Status info row (above input, non-interactive) */}
                {(connectionStatus || onClear || todoToggle) && (
                    <View style={styles.statusInfoRow}>
                        {connectionStatus && (
                            <>
                                <StatusDot
                                    color={connectionStatus.dotColor}
                                    isPulsing={connectionStatus.isPulsing}
                                    size={6}
                                />
                                <Text style={[styles.statusText, { color: connectionStatus.color }]}>
                                    {connectionStatus.text}
                                </Text>
                                {connectionStatus.compressionInfo && (
                                    <Text style={[styles.statusText, { color: connectionStatus.compressionInfo.color, marginLeft: 4 }]}>
                                        • context: {connectionStatus.compressionInfo.text}
                                    </Text>
                                )}
                                {quotaVendor && (
                                    <QuotaPopover
                                        machineId={quotaMachineId ?? null}
                                        vendor={quotaVendor}
                                        preferredModelId={quotaPreferredModelId ?? null}
                                    />
                                )}
                            </>
                        )}
                        {todoToggle && (
                            <Pressable
                                onPress={() => {
                                    hapticsLight();
                                    todoToggle.onPress();
                                }}
                                hitSlop={{ top: 6, bottom: 6, left: 6, right: 6 }}
                                accessibilityLabel={todoToggle.collapsed ? '展开 todolist' : '收起 todolist'}
                                style={({ pressed }) => [
                                    styles.todoStatusButton,
                                    { opacity: pressed ? 0.6 : 1 },
                                ]}
                            >
                                <Text style={[styles.statusText, { color: theme.colors.textSecondary, ...Typography.default('semiBold') }]}>
                                    • Todo {todoToggle.completed}/{todoToggle.total}
                                </Text>
                                <Ionicons
                                    name={todoToggle.collapsed ? 'chevron-up' : 'chevron-down'}
                                    size={12}
                                    color={theme.colors.textSecondary}
                                />
                            </Pressable>
                        )}
                        {onClear && (
                            <Pressable
                                onPress={async () => {
                                    hapticsLight();
                                    const confirmed = await Modal.confirm('清空聊天', '将清空当前聊天历史，此操作不可撤销。');
                                    if (confirmed) onClear();
                                }}
                                disabled={isClearLoading}
                                hitSlop={{ top: 5, bottom: 5, left: 5, right: 5 }}
                                accessibilityLabel="清空当前聊天历史"
                                style={(p) => ({
                                    flexDirection: 'row',
                                    alignItems: 'center',
                                    marginLeft: 'auto' as any,
                                    opacity: isClearLoading ? 0.4 : p.pressed ? 0.6 : 1,
                                })}
                            >
                                {isClearLoading ? (
                                    <ActivityIndicator size={12} color={theme.colors.textSecondary} />
                                ) : (
                                    <Ionicons name="brush-outline" size={14} color={theme.colors.textSecondary} />
                                )}
                                <Text style={[styles.statusText, { color: theme.colors.textSecondary, marginLeft: 3 }]}>清空聊天</Text>
                            </Pressable>
                        )}
                    </View>
                )}

                <View style={styles.unifiedPanel}>
                    {pendingPermission && sessionId && (
                        <PermissionRequestCard
                            sessionId={sessionId}
                            pendingPermission={pendingPermission}
                            metadata={metadata}
                            variant="card"
                        />
                    )}

                    {/* Image preview row */}
                    {hasImages && (
                        <ScrollView
                            horizontal
                            showsHorizontalScrollIndicator={false}
                            style={{ paddingHorizontal: 8, paddingTop: 8 }}
                            contentContainerStyle={{ gap: 8 }}
                        >
                            {pendingImages!.map((img, idx) => (
                                <View key={idx} style={{ position: 'relative' }}>
                                    <Image
                                        source={{ uri: img.uri }}
                                        style={{ width: 64, height: 64, borderRadius: 8 }}
                                        contentFit="cover"
                                    />
                                    <Pressable
                                        onPress={() => onRemoveImage?.(idx)}
                                        style={{
                                            position: 'absolute',
                                            top: -6,
                                            right: -6,
                                            width: 20,
                                            height: 20,
                                            borderRadius: 10,
                                            backgroundColor: theme.colors.text,
                                            alignItems: 'center',
                                            justifyContent: 'center',
                                        }}
                                        hitSlop={4}
                                    >
                                        <Ionicons name="close" size={12} color={theme.colors.surface} />
                                    </Pressable>
                                </View>
                            ))}
                        </ScrollView>
                    )}

                    {/* Selected skill tags */}
                    {selectedSkills && selectedSkills.length > 0 && (
                        <View style={{ flexDirection: 'row', flexWrap: 'wrap', gap: 6, paddingHorizontal: 10, paddingTop: 8 }}>
                            {selectedSkills.map((skill) => (
                                <View
                                    key={skill.id}
                                    style={{
                                        flexDirection: 'row',
                                        alignItems: 'center',
                                        backgroundColor: theme.colors.textLink + '18',
                                        borderRadius: 12,
                                        paddingLeft: 8,
                                        paddingRight: 4,
                                        paddingVertical: 3,
                                    }}
                                >
                                    <Text style={{
                                        fontSize: 13,
                                        color: theme.colors.textLink,
                                        ...Typography.default('semiBold'),
                                    }}>
                                        @{skill.name}
                                    </Text>
                                    <Pressable
                                        onPress={() => onRemoveSkill?.(skill.id)}
                                        hitSlop={4}
                                        style={{ marginLeft: 2, padding: 2 }}
                                    >
                                        <Ionicons name="close-circle" size={14} color={theme.colors.textLink} />
                                    </Pressable>
                                </View>
                            ))}
                        </View>
                    )}

                    {/* Input field */}
                    <View style={styles.inputContainer}>
                        <MultiTextInput
                            ref={inputRef}
                            value={value}
                            onChangeText={onChangeText}
                            placeholder={placeholder}
                            maxHeight={120}
                            paddingTop={Platform.OS === 'web' ? 10 : 8}
                            paddingBottom={Platform.OS === 'web' ? 10 : 8}
                            onKeyPress={handleKeyPress}
                            onSelectionChange={setSelection}
                            onStateChange={(state) => setSelection(state.selection)}
                            editable={!isPermissionBlocked}
                        />
                    </View>

                    {autocompleteSuggestions.length > 0 && (
                        <View style={{ paddingHorizontal: 10, paddingBottom: 6, gap: 6 }}>
                            <ScrollView
                                horizontal
                                showsHorizontalScrollIndicator={false}
                                contentContainerStyle={{ gap: 8 }}
                                keyboardShouldPersistTaps="always"
                            >
                                {autocompleteSuggestions.map((option, index) => {
                                    const isSelected = index === selectedAutocompleteIndex;
                                    return (
                                        <Pressable
                                            key={option.id}
                                            onPress={() => {
                                                hapticsLight();
                                                applyAutocompleteSuggestion(option.id);
                                            }}
                                            style={{
                                                flexDirection: 'row',
                                                alignItems: 'center',
                                                gap: 6,
                                                paddingHorizontal: 10,
                                                paddingVertical: 6,
                                                borderRadius: 999,
                                                backgroundColor: isSelected ? theme.colors.button.primary.background : theme.colors.surface,
                                                borderWidth: 1,
                                                borderColor: isSelected ? theme.colors.button.primary.background : theme.colors.divider,
                                            }}
                                        >
                                            <Text style={{
                                                fontSize: 12,
                                                color: isSelected ? theme.colors.button.primary.tint : theme.colors.text,
                                                ...Typography.default('semiBold'),
                                            }}>
                                                @{option.id}
                                            </Text>
                                            {!!option.description && (
                                                <Text style={{
                                                    fontSize: 11,
                                                    color: isSelected ? theme.colors.button.primary.tint : theme.colors.textSecondary,
                                                    ...Typography.default(),
                                                }}>
                                                    {option.description}
                                                </Text>
                                            )}
                                        </Pressable>
                                    );
                                })}
                            </ScrollView>
                        </View>
                    )}

                    {/* Action buttons below input */}
                    <View style={styles.actionButtonsContainer}>
                        {/* Interactive buttons (left side) */}
                        <View style={styles.statusRow}>
                            {/* Image picker button */}
                            {supportsVision && onAddImage && (
                                <Pressable
                                    onPress={() => {
                                        hapticsLight();
                                        handlePickImage();
                                    }}
                                    hitSlop={{ top: 5, bottom: 10, left: 0, right: 0 }}
                                    style={(p) => ({
                                        flexDirection: 'row',
                                        alignItems: 'center',
                                        marginLeft: 8,
                                        opacity: p.pressed ? 0.6 : 1,
                                    })}
                                >
                                    <Ionicons
                                        name="image-outline"
                                        size={13}
                                        color={hasImages ? theme.colors.button.primary.background : theme.colors.textSecondary}
                                        style={{ marginRight: 3 }}
                                    />
                                    {hasImages && (
                                        <Text style={[styles.statusText, { color: theme.colors.button.primary.background, ...Typography.default('semiBold') }]}>
                                            {pendingImages!.length}
                                        </Text>
                                    )}
                                </Pressable>
                            )}
                            {/* Skill selector button */}
                            {onSkillClick && (
                                <Pressable
                                    onPress={() => {
                                        hapticsLight();
                                        onSkillClick();
                                    }}
                                    hitSlop={{ top: 5, bottom: 10, left: 0, right: 0 }}
                                    style={(p) => ({
                                        flexDirection: 'row',
                                        alignItems: 'center',
                                        marginLeft: 8,
                                        opacity: p.pressed ? 0.6 : 1,
                                    })}
                                >
                                    <Ionicons
                                        name="extension-puzzle-outline"
                                        size={13}
                                        color={theme.colors.textSecondary}
                                        style={{ marginRight: 3 }}
                                    />
                                    <Text style={[styles.statusText, { color: theme.colors.textSecondary, ...Typography.default('semiBold') }]}>
                                        Skills ({activeSkillCount ?? 0})
                                    </Text>
                                </Pressable>
                            )}
                            {/* Agent list button */}
                            {onAgentClick && (
                                <Pressable
                                    onPress={() => {
                                        hapticsLight();
                                        onAgentClick();
                                    }}
                                    hitSlop={{ top: 5, bottom: 10, left: 0, right: 0 }}
                                    style={(p) => ({
                                        flexDirection: 'row',
                                        alignItems: 'center',
                                        marginLeft: 8,
                                        opacity: p.pressed ? 0.6 : 1,
                                    })}
                                >
                                    <Ionicons
                                        name="cube-outline"
                                        size={13}
                                        color={theme.colors.textSecondary}
                                        style={{ marginRight: 3 }}
                                    />
                                    <Text style={[styles.statusText, { color: theme.colors.textSecondary, ...Typography.default('semiBold') }]}>
                                        Agents ({agentCount ?? 0})
                                    </Text>
                                </Pressable>
                            )}
                            {/* Model switcher button */}
                            {modelName && onModelPress && (
                                <Pressable
                                    onPress={() => {
                                        hapticsLight();
                                        onModelPress();
                                    }}
                                    hitSlop={{ top: 5, bottom: 10, left: 0, right: 0 }}
                                    style={(p) => ({
                                        flexDirection: 'row',
                                        alignItems: 'center',
                                        marginLeft: 8,
                                        opacity: p.pressed ? 0.6 : 1,
                                    })}
                                >
                                    <Ionicons
                                        name="sparkles-outline"
                                        size={13}
                                        color={theme.colors.textSecondary}
                                        style={{ marginRight: 3 }}
                                    />
                                    <Text style={[styles.statusText, { color: theme.colors.textSecondary }]} numberOfLines={1}>
                                        {modelName}
                                    </Text>
                                </Pressable>
                            )}
                            {effortName && onEffortPress && (
                                <Pressable
                                    onPress={() => {
                                        hapticsLight();
                                        onEffortPress();
                                    }}
                                    hitSlop={{ top: 5, bottom: 10, left: 0, right: 0 }}
                                    style={(p) => ({
                                        flexDirection: 'row',
                                        alignItems: 'center',
                                        marginLeft: 8,
                                        opacity: p.pressed ? 0.6 : 1,
                                    })}
                                >
                                    <Ionicons
                                        name="flask-outline"
                                        size={13}
                                        color={theme.colors.textSecondary}
                                        style={{ marginRight: 3 }}
                                    />
                                    <Text style={[styles.statusText, { color: theme.colors.textSecondary }]} numberOfLines={1}>
                                        {effortName}
                                    </Text>
                                </Pressable>
                            )}
                        </View>

                        {/* Mic / Stop-recording button */}
                        {onMicPress && (
                            <View
                                style={[
                                    styles.sendButton,
                                    isMicActive
                                        ? { backgroundColor: '#FF3B30' }
                                        : styles.sendButtonActive
                                ]}
                            >
                                <Pressable
                                    onPress={() => {
                                        hapticsLight();
                                        onMicPress?.();
                                    }}
                                    style={(p) => ({
                                        width: '100%',
                                        height: '100%',
                                        alignItems: 'center',
                                        justifyContent: 'center',
                                        opacity: p.pressed ? 0.7 : 1,
                                    })}
                                    hitSlop={{ top: 5, bottom: 10, left: 0, right: 0 }}
                                >
                                    {isMicActive ? (
                                        <Ionicons name="stop" size={16} color="#FFFFFF" />
                                    ) : (
                                        <Image
                                            source={require('@/assets/images/icon-voice-white.png')}
                                            style={{ width: 24, height: 24 }}
                                            tintColor={theme.colors.button.primary.tint}
                                        />
                                    )}
                                </Pressable>
                            </View>
                        )}

                        {/* Send / Abort button */}
                        <View
                            style={[
                                styles.sendButton,
                                (canSend || (showAbortButton && !hasContent))
                                    ? styles.sendButtonActive
                                    : styles.sendButtonInactive
                            ]}
                        >
                            {showAbortButton && !hasContent && onAbort ? (
                                <Pressable
                                    style={(p) => ({
                                        width: '100%',
                                        height: '100%',
                                        alignItems: 'center',
                                        justifyContent: 'center',
                                        opacity: p.pressed ? 0.7 : 1,
                                    })}
                                    hitSlop={{ top: 5, bottom: 10, left: 0, right: 0 }}
                                    onPress={handleAbortPress}
                                    disabled={isAborting}
                                >
                                    {isAborting ? (
                                        <ActivityIndicator
                                            size="small"
                                            color={theme.colors.button.primary.tint}
                                        />
                                    ) : (
                                        <Ionicons
                                            name="stop"
                                            size={18}
                                            color={theme.colors.button.primary.tint}
                                        />
                                    )}
                                </Pressable>
                            ) : (
                                <Pressable
                                    style={(p) => ({
                                        width: '100%',
                                        height: '100%',
                                        alignItems: 'center',
                                        justifyContent: 'center',
                                        opacity: p.pressed ? 0.7 : 1,
                                    })}
                                    hitSlop={{ top: 5, bottom: 10, left: 0, right: 0 }}
                                    onPress={() => {
                                        if (isPermissionBlocked) return;
                                        hapticsLight();
                                        onSend();
                                    }}
                                    disabled={!canSend}
                                >
                                    <Octicons
                                        name="arrow-up"
                                        size={16}
                                        color={theme.colors.button.primary.tint}
                                        style={[
                                            styles.sendButtonIcon,
                                            { marginTop: Platform.OS === 'web' ? 2 : 0 }
                                        ]}
                                    />
                                </Pressable>
                            )}
                        </View>
                    </View>
                </View>

            </View>
        </View>
    );
});

PersonaChatInput.displayName = 'PersonaChatInput';
