import React, { useState, useCallback, useEffect, useMemo, useRef } from 'react';
import { View, ScrollView, Pressable, ActivityIndicator, useWindowDimensions, Modal as RNModal, TextInput } from 'react-native';
import { Modal } from '@/modal';
import { Switch } from '@/components/Switch';
import { Ionicons } from '@expo/vector-icons';
import { useUnistyles } from 'react-native-unistyles';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { Typography } from '@/constants/Typography';
import { Text } from '@/components/StyledText';
import { useAllMachines, useLocalSettingMutable } from '@/sync/storage';
import { isMachineOnline } from '@/utils/machineUtils';
import {
    machineListSkills, machineDeleteSkill, type SkillListItem,
    machineSkillhubFeatured, machineSkillhubSearch, machineSkillhubInstall,
    type SkillHubItem,
} from '@/sync/ops';
import { t } from '@/text';
import { invoke } from '@tauri-apps/api/core';

// Stable color palette for skill icons
const SKILL_COLORS = [
    '#FF6B6B', '#4ECDC4', '#45B7D1', '#96CEB4',
    '#FFEAA7', '#DDA0DD', '#98D8C8', '#F7DC6F',
    '#BB8FCE', '#85C1E9', '#F0B27A', '#82E0AA',
    '#F1948A', '#AED6F1', '#A3E4D7', '#FAD7A0',
];

const SKILL_ICONS: Record<string, { name: string; color: string }> = {
    'shell': { name: 'terminal-outline', color: '#4ECDC4' },
    'file': { name: 'document-outline', color: '#45B7D1' },
    'edit': { name: 'create-outline', color: '#F7DC6F' },
    'read': { name: 'reader-outline', color: '#82E0AA' },
    'websearch': { name: 'globe-outline', color: '#85C1E9' },
    'memory': { name: 'brain-outline', color: '#DDA0DD' },
    'image': { name: 'image-outline', color: '#F0B27A' },
    'pdf': { name: 'document-text-outline', color: '#FF6B6B' },
    'docx': { name: 'document-text-outline', color: '#5DADE2' },
    'code': { name: 'code-slash-outline', color: '#96CEB4' },
    'social': { name: 'share-social-outline', color: '#BB8FCE' },
    'pptx': { name: 'easel-outline', color: '#F1948A' },
    'xlsx': { name: 'grid-outline', color: '#82E0AA' },
    'frontend': { name: 'color-palette-outline', color: '#45B7D1' },
    'remotion': { name: 'videocam-outline', color: '#FF6B6B' },
    'installer': { name: 'download-outline', color: '#F7DC6F' },
    'creator': { name: 'hammer-outline', color: '#F0B27A' },
    'compress': { name: 'resize-outline', color: '#96CEB4' },
    'search': { name: 'search-outline', color: '#85C1E9' },
    'scout': { name: 'compass-outline', color: '#BB8FCE' },
    'wechat': { name: 'chatbubbles-outline', color: '#07C160' },
    'claude': { name: 'sparkles-outline', color: '#AED6F1' },
    'flow': { name: 'git-branch-outline', color: '#FFEAA7' },
    'aspnet': { name: 'server-outline', color: '#BB8FCE' },
    'chatgpt': { name: 'chatbox-outline', color: '#74AA9C' },
};

function getSkillIcon(skill: { id: string } | { slug: string }): { name: string; color: string } {
    const id = ('id' in skill ? skill.id : skill.slug).toLowerCase();
    for (const [key, val] of Object.entries(SKILL_ICONS)) {
        if (id.includes(key)) return val;
    }
    let hash = 0;
    for (let i = 0; i < id.length; i++) {
        hash = ((hash << 5) - hash + id.charCodeAt(i)) | 0;
    }
    return {
        name: 'flash-outline',
        color: SKILL_COLORS[Math.abs(hash) % SKILL_COLORS.length],
    };
}

function formatDownloads(n: number): string {
    if (n >= 10000) return `${(n / 10000).toFixed(1)}w`;
    if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
    return String(n);
}

// ==================== Skill Detail Modal ====================

function SkillDetailModal({ skill, visible, onClose, onDisable, onDelete, enabled, theme }: {
    skill: SkillListItem | null;
    visible: boolean;
    onClose: () => void;
    onDisable: () => void;
    onDelete: () => void;
    enabled: boolean;
    theme: any;
}) {
    if (!skill) return null;

    const icon = getSkillIcon(skill);
    const isBuiltin = skill.source === 'builtin';

    const openFolder = () => {
        if (skill.path) {
            invoke('open_url', { url: skill.path }).catch(console.warn);
        }
    };

    return (
        <RNModal visible={visible} transparent animationType="fade" onRequestClose={onClose}>
            <Pressable
                style={{ flex: 1, backgroundColor: 'rgba(0,0,0,0.5)', justifyContent: 'center', alignItems: 'center' }}
                onPress={onClose}
            >
                <Pressable
                    onPress={(e) => e.stopPropagation()}
                    style={{
                        backgroundColor: theme.colors.groupped.background,
                        borderRadius: 16,
                        width: '90%',
                        maxWidth: 520,
                        maxHeight: '85%',
                        overflow: 'hidden',
                    }}
                >
                    <View style={{
                        flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between',
                        paddingHorizontal: 20, paddingTop: 20, paddingBottom: 12,
                    }}>
                        <View style={{
                            width: 44, height: 44, borderRadius: 12,
                            backgroundColor: icon.color + '20', alignItems: 'center', justifyContent: 'center',
                        }}>
                            <Ionicons name={icon.name as any} size={24} color={icon.color} />
                        </View>
                        <Pressable onPress={onClose} hitSlop={15}>
                            <Ionicons name="close" size={24} color={theme.colors.text} />
                        </Pressable>
                    </View>

                    <View style={{ paddingHorizontal: 20, marginBottom: 16 }}>
                        <Text style={{ fontSize: 22, color: theme.colors.text, ...Typography.default('semiBold') }}>
                            {skill.name}
                        </Text>
                        <Text style={{ fontSize: 14, color: theme.colors.textSecondary, marginTop: 4, ...Typography.default() }}>
                            {skill.description}
                        </Text>
                        {/* Badges: scripts, version */}
                        <View style={{ flexDirection: 'row', gap: 8, marginTop: 8, flexWrap: 'wrap' }}>
                            {skill.hasScripts && (
                                <View style={{
                                    flexDirection: 'row', alignItems: 'center', gap: 4,
                                    paddingHorizontal: 8, paddingVertical: 3, borderRadius: 6,
                                    backgroundColor: '#14b8a620',
                                }}>
                                    <Ionicons name="code-slash-outline" size={12} color="#14b8a6" />
                                    <Text style={{ fontSize: 11, color: '#14b8a6', ...Typography.default('semiBold') }}>scripts</Text>
                                </View>
                            )}
                            {skill.version ? (
                                <View style={{
                                    paddingHorizontal: 8, paddingVertical: 3, borderRadius: 6,
                                    backgroundColor: theme.colors.surfaceHigh,
                                }}>
                                    <Text style={{ fontSize: 11, color: theme.colors.textSecondary, ...Typography.default() }}>v{skill.version}</Text>
                                </View>
                            ) : null}
                            <View style={{
                                paddingHorizontal: 8, paddingVertical: 3, borderRadius: 6,
                                backgroundColor: theme.colors.surfaceHigh,
                            }}>
                                <Text style={{ fontSize: 11, color: theme.colors.textSecondary, ...Typography.default() }}>
                                    {isBuiltin ? t('skills.builtin') : t('skills.installed')}
                                </Text>
                            </View>
                        </View>
                    </View>

                    <ScrollView style={{ maxHeight: 400 }} contentContainerStyle={{ paddingHorizontal: 20, paddingBottom: 16 }}>
                        {skill.instructions ? (
                            <View style={{ backgroundColor: theme.colors.surfaceHigh, borderRadius: 12, padding: 16 }}>
                                <Text style={{ fontSize: 13, color: theme.colors.text, lineHeight: 20, ...Typography.default() }}>
                                    {skill.instructions}
                                </Text>
                            </View>
                        ) : null}
                    </ScrollView>

                    <View style={{
                        flexDirection: 'row', paddingHorizontal: 20, paddingTop: 12, paddingBottom: 20,
                        borderTopWidth: 1, borderTopColor: theme.colors.divider, gap: 8,
                        alignItems: 'center',
                    }}>
                        {!isBuiltin && (
                            <Pressable
                                onPress={onDelete}
                                style={({ pressed }) => ({
                                    paddingHorizontal: 14, paddingVertical: 8, borderRadius: 10,
                                    backgroundColor: pressed ? '#FF3B3020' : 'transparent',
                                })}
                            >
                                <Ionicons name="trash-outline" size={18} color="#FF3B30" />
                            </Pressable>
                        )}
                        {skill.path && (
                            <Pressable
                                onPress={openFolder}
                                style={({ pressed }) => ({
                                    paddingHorizontal: 14, paddingVertical: 8, borderRadius: 10,
                                    backgroundColor: pressed ? theme.colors.surfacePressed : 'transparent',
                                })}
                            >
                                <Ionicons name="folder-open-outline" size={18} color={theme.colors.text} />
                            </Pressable>
                        )}
                        <View style={{ flex: 1 }} />
                        <Pressable
                            onPress={onDisable}
                            style={({ pressed }) => ({
                                paddingHorizontal: 16, paddingVertical: 8, borderRadius: 10,
                                backgroundColor: pressed ? theme.colors.surfacePressed : theme.colors.surfaceHigh,
                            })}
                        >
                            <Text style={{ fontSize: 14, color: theme.colors.text, ...Typography.default('semiBold') }}>
                                {enabled ? t('skills.disable') : t('skills.enable')}
                            </Text>
                        </Pressable>
                    </View>
                </Pressable>
            </Pressable>
        </RNModal>
    );
}

// ==================== Skill Card (local) ====================

function SkillCard({ skill, enabled, onToggle, onPress, theme }: {
    skill: SkillListItem;
    enabled: boolean;
    onToggle: (val: boolean) => void;
    onPress: () => void;
    theme: any;
}) {
    const icon = getSkillIcon(skill);

    return (
        <Pressable
            onPress={onPress}
            style={({ pressed }) => ({
                flexDirection: 'row', alignItems: 'center', padding: 12, borderRadius: 12,
                backgroundColor: pressed ? theme.colors.surfacePressed : theme.colors.surfaceHigh,
            })}
        >
            <View style={{
                width: 40, height: 40, borderRadius: 10,
                backgroundColor: icon.color + '20', alignItems: 'center', justifyContent: 'center',
                marginRight: 10, flexShrink: 0,
            }}>
                <Ionicons name={icon.name as any} size={20} color={icon.color} />
            </View>
            <View style={{ flex: 1, marginRight: 8 }}>
                <View style={{ flexDirection: 'row', alignItems: 'center', gap: 6 }}>
                    <Text style={{ fontSize: 14, color: theme.colors.text, flexShrink: 1, ...Typography.default('semiBold') }} numberOfLines={1}>
                        {skill.name}
                    </Text>
                    {skill.source !== 'builtin' && (
                        <View style={{
                            flexDirection: 'row', alignItems: 'center',
                            paddingHorizontal: 5, paddingVertical: 1, borderRadius: 4,
                            borderWidth: 1, borderColor: theme.colors.divider,
                        }}>
                            <Ionicons name="cube-outline" size={10} color={theme.colors.textSecondary} />
                            <Text style={{ fontSize: 10, color: theme.colors.textSecondary, marginLeft: 2, ...Typography.default() }}>Cteno</Text>
                        </View>
                    )}
                </View>
                <Text style={{ fontSize: 12, color: theme.colors.textSecondary, marginTop: 2, ...Typography.default() }} numberOfLines={1}>
                    {skill.description}
                </Text>
            </View>
            <Pressable onPress={(e) => { e.stopPropagation(); onToggle(!enabled); }} hitSlop={8}>
                <Switch value={enabled} onValueChange={onToggle} pointerEvents="none" />
            </Pressable>
        </Pressable>
    );
}

// ==================== SkillHub Card (store) ====================

function SkillHubCard({ item, onInstall, installing, theme }: {
    item: SkillHubItem;
    onInstall: (slug: string, displayName?: string) => void;
    installing: string | null;
    theme: any;
}) {
    const icon = getSkillIcon({ slug: item.slug } as any);
    const isInstalling = installing === item.slug;

    return (
        <View style={{
            padding: 12, borderRadius: 12,
            backgroundColor: theme.colors.surfaceHigh,
        }}>
            <View style={{ flexDirection: 'row', alignItems: 'center' }}>
                <View style={{
                    width: 40, height: 40, borderRadius: 10,
                    backgroundColor: icon.color + '20', alignItems: 'center', justifyContent: 'center',
                    marginRight: 10, flexShrink: 0,
                }}>
                    <Ionicons name={icon.name as any} size={20} color={icon.color} />
                </View>
                <View style={{ flex: 1, marginRight: 8 }}>
                    <Text style={{ fontSize: 14, color: theme.colors.text, ...Typography.default('semiBold') }} numberOfLines={1}>
                        {item.name}
                    </Text>
                    {item.stats.downloads > 0 && (
                        <View style={{ flexDirection: 'row', alignItems: 'center', gap: 4, marginTop: 2 }}>
                            <Ionicons name="download-outline" size={11} color={theme.colors.textSecondary} />
                            <Text style={{ fontSize: 11, color: theme.colors.textSecondary, ...Typography.default() }}>
                                {formatDownloads(item.stats.downloads)}
                            </Text>
                        </View>
                    )}
                </View>
                {item.installed ? (
                    <View style={{
                        paddingHorizontal: 10, paddingVertical: 6, borderRadius: 8,
                        backgroundColor: theme.colors.surfaceHigh,
                        borderWidth: 1, borderColor: theme.colors.divider,
                    }}>
                        <Text style={{ fontSize: 12, color: theme.colors.textSecondary, ...Typography.default('semiBold') }}>
                            {t('skills.installed')}
                        </Text>
                    </View>
                ) : (
                    <Pressable
                        onPress={() => onInstall(item.slug, item.name)}
                        disabled={isInstalling}
                        style={({ pressed }) => ({
                            paddingHorizontal: 12, paddingVertical: 6, borderRadius: 8,
                            backgroundColor: pressed ? '#0f766e' : '#14b8a6',
                            opacity: isInstalling ? 0.6 : 1,
                        })}
                    >
                        {isInstalling ? (
                            <ActivityIndicator size="small" color="#fff" />
                        ) : (
                            <Text style={{ fontSize: 12, color: '#fff', ...Typography.default('semiBold') }}>
                                {t('skills.install')}
                            </Text>
                        )}
                    </Pressable>
                )}
            </View>
            <Text style={{
                fontSize: 12, color: theme.colors.textSecondary, marginTop: 8, lineHeight: 17,
                ...Typography.default(),
            }} numberOfLines={2}>
                {item.description}
            </Text>
        </View>
    );
}

// ==================== Main Page ====================

export default function SkillsPage() {
    const { theme } = useUnistyles();
    const insets = useSafeAreaInsets();
    const { width: windowWidth } = useWindowDimensions();
    const machines = useAllMachines();
    const [selectedMachineIdFilter] = useLocalSettingMutable('selectedMachineIdFilter');

    const machineId = useMemo(() => {
        if (selectedMachineIdFilter) return selectedMachineIdFilter;
        const online = machines.find(m => isMachineOnline(m));
        return online?.id || (machines.length > 0 ? machines[0].id : undefined);
    }, [selectedMachineIdFilter, machines]);

    // Local skills state
    const [skills, setSkills] = useState<SkillListItem[]>([]);
    const [loading, setLoading] = useState(false);
    const [enabledIds, setEnabledIds] = useState<Set<string>>(new Set());
    const [detailSkill, setDetailSkill] = useState<SkillListItem | null>(null);

    // SkillHub state
    const [featuredSkills, setFeaturedSkills] = useState<SkillHubItem[]>([]);
    const [searchResults, setSearchResults] = useState<SkillHubItem[] | null>(null);
    const [searchQuery, setSearchQuery] = useState('');
    const [searchLoading, setSearchLoading] = useState(false);
    const [featuredLoading, setFeaturedLoading] = useState(false);
    const [installing, setInstalling] = useState<string | null>(null);
    const searchTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

    const loadSkills = useCallback(async () => {
        if (!machineId) return;
        setLoading(true);
        try {
            const result = await machineListSkills(machineId);
            const loaded = result.skills || [];
            setSkills(loaded);
            setEnabledIds(new Set(loaded.map(s => s.id)));
        } catch (e) {
            console.warn('Failed to load skills:', e);
        } finally {
            setLoading(false);
        }
    }, [machineId]);

    const loadFeatured = useCallback(async () => {
        if (!machineId) return;
        setFeaturedLoading(true);
        try {
            const result = await machineSkillhubFeatured(machineId);
            setFeaturedSkills(result.skills || []);
        } catch (e) {
            console.warn('Failed to load featured:', e);
        } finally {
            setFeaturedLoading(false);
        }
    }, [machineId]);

    useEffect(() => { loadSkills(); loadFeatured(); }, [loadSkills, loadFeatured]);

    const handleSearch = useCallback(async (q: string) => {
        if (!machineId) return;
        if (!q.trim()) {
            setSearchResults(null);
            return;
        }
        setSearchLoading(true);
        try {
            const result = await machineSkillhubSearch(machineId, q.trim());
            setSearchResults(result.skills || []);
        } catch (e) {
            console.warn('SkillHub search error:', e);
            setSearchResults([]);
        } finally {
            setSearchLoading(false);
        }
    }, [machineId]);

    const onSearchChange = useCallback((text: string) => {
        setSearchQuery(text);
        if (searchTimer.current) clearTimeout(searchTimer.current);
        if (!text.trim()) {
            setSearchResults(null);
            return;
        }
        searchTimer.current = setTimeout(() => handleSearch(text), 500);
    }, [handleSearch]);

    const handleInstall = useCallback(async (slug: string, displayName?: string) => {
        if (!machineId || installing) return;
        setInstalling(slug);
        try {
            const result = await machineSkillhubInstall(machineId, slug, displayName);
            if (result.success) {
                // Mark as installed in both lists
                setFeaturedSkills(prev => prev.map(s => s.slug === slug ? { ...s, installed: true } : s));
                setSearchResults(prev => prev ? prev.map(s => s.slug === slug ? { ...s, installed: true } : s) : prev);
                // Reload local skills
                await loadSkills();
            } else {
                Modal.confirm('Install Failed', result.error || 'Unknown error');
            }
        } catch (e) {
            Modal.confirm('Install Failed', e instanceof Error ? e.message : 'Unknown error');
        } finally {
            setInstalling(null);
        }
    }, [machineId, installing, loadSkills]);

    const handleToggle = useCallback((skillId: string, val: boolean) => {
        setEnabledIds(prev => {
            const next = new Set(prev);
            if (val) next.add(skillId); else next.delete(skillId);
            return next;
        });
    }, []);

    const handleDelete = useCallback(async (skill: SkillListItem) => {
        if (!machineId) return;
        const confirmed = await Modal.confirm(
            t('skills.deleteConfirmTitle'),
            t('skills.deleteConfirmMessage'),
            { confirmText: t('common.delete'), destructive: true }
        );
        if (!confirmed) return;
        const result = await machineDeleteSkill(machineId, skill.id);
        if (result.success) {
            setDetailSkill(null);
            await loadSkills();
            loadFeatured();
        }
    }, [machineId, loadSkills, loadFeatured]);

    const isNarrow = windowWidth < 500;
    const installedSkills = skills.filter(s => s.source !== 'builtin');
    const builtinSkills = skills.filter(s => s.source === 'builtin');

    // Which SkillHub list to show: search results or featured
    const hubSkills = searchResults ?? featuredSkills;
    const showingSearch = searchResults !== null;

    return (
        <View style={{ flex: 1, backgroundColor: theme.colors.groupped.background }}>
            <ScrollView
                style={{ flex: 1 }}
                contentContainerStyle={{
                    paddingTop: insets.top + 16,
                    paddingHorizontal: 16,
                    paddingBottom: insets.bottom + 32,
                }}
            >
                {/* Title */}
                <Text style={{ fontSize: 28, color: theme.colors.text, marginBottom: 4, ...Typography.default('semiBold') }}>
                    {t('skills.title')}
                </Text>
                <Text style={{ fontSize: 15, color: theme.colors.textSecondary, marginBottom: 24, ...Typography.default() }}>
                    {t('skills.subtitle')}
                </Text>

                {loading && skills.length === 0 ? (
                    <View style={{ paddingVertical: 40, alignItems: 'center' }}>
                        <ActivityIndicator size="small" color={theme.colors.textSecondary} />
                    </View>
                ) : (
                    <>
                        {/* Installed Skills */}
                        {installedSkills.length > 0 && (
                            <>
                                <Text style={{ fontSize: 13, color: theme.colors.textSecondary, marginBottom: 10, ...Typography.default('semiBold') }}>
                                    {t('skills.installed')}
                                </Text>
                                <View style={{ flexDirection: 'row', flexWrap: 'wrap', gap: 8, marginBottom: 24 }}>
                                    {installedSkills.map(skill => (
                                        <View key={skill.id} style={{ width: isNarrow ? '100%' : '48.5%' }}>
                                            <SkillCard
                                                skill={skill} enabled={enabledIds.has(skill.id)}
                                                onToggle={(val) => handleToggle(skill.id, val)}
                                                onPress={() => setDetailSkill(skill)} theme={theme}
                                            />
                                        </View>
                                    ))}
                                </View>
                            </>
                        )}

                        {/* Builtin Skills */}
                        {builtinSkills.length > 0 && (
                            <>
                                <Text style={{ fontSize: 13, color: theme.colors.textSecondary, marginBottom: 10, ...Typography.default('semiBold') }}>
                                    {t('skills.builtin')}
                                </Text>
                                <View style={{ flexDirection: 'row', flexWrap: 'wrap', gap: 8, marginBottom: 24 }}>
                                    {builtinSkills.map(skill => (
                                        <View key={skill.id} style={{ width: isNarrow ? '100%' : '48.5%' }}>
                                            <SkillCard
                                                skill={skill} enabled={enabledIds.has(skill.id)}
                                                onToggle={(val) => handleToggle(skill.id, val)}
                                                onPress={() => setDetailSkill(skill)} theme={theme}
                                            />
                                        </View>
                                    ))}
                                </View>
                            </>
                        )}

                        {/* SkillHub Store Section */}
                        <View style={{
                            marginTop: 8, paddingTop: 20,
                            borderTopWidth: 1, borderTopColor: theme.colors.divider,
                        }}>
                            <View style={{ flexDirection: 'row', alignItems: 'center', marginBottom: 12 }}>
                                <Ionicons name="storefront-outline" size={18} color={theme.colors.text} />
                                <Text style={{ fontSize: 16, color: theme.colors.text, marginLeft: 6, ...Typography.default('semiBold') }}>
                                    SkillHub
                                </Text>
                                <Text style={{ fontSize: 12, color: theme.colors.textSecondary, marginLeft: 8, ...Typography.default() }}>
                                    12000+ skills
                                </Text>
                            </View>

                            {/* Search Bar */}
                            <View style={{
                                flexDirection: 'row', alignItems: 'center',
                                backgroundColor: theme.colors.surfaceHigh,
                                borderRadius: 10, paddingHorizontal: 12, marginBottom: 16,
                                borderWidth: 1, borderColor: theme.colors.divider,
                            }}>
                                <Ionicons name="search-outline" size={18} color={theme.colors.textSecondary} />
                                <TextInput
                                    value={searchQuery}
                                    onChangeText={onSearchChange}
                                    placeholder={t('skills.searchPlaceholder')}
                                    placeholderTextColor={theme.colors.textSecondary}
                                    style={{
                                        flex: 1, paddingVertical: 10, paddingHorizontal: 8,
                                        fontSize: 14, color: theme.colors.text,
                                        ...Typography.default(),
                                    }}
                                    returnKeyType="search"
                                    onSubmitEditing={() => handleSearch(searchQuery)}
                                />
                                {searchQuery.length > 0 && (
                                    <Pressable onPress={() => { setSearchQuery(''); setSearchResults(null); }} hitSlop={8}>
                                        <Ionicons name="close-circle" size={18} color={theme.colors.textSecondary} />
                                    </Pressable>
                                )}
                            </View>

                            {/* Section label */}
                            <Text style={{ fontSize: 13, color: theme.colors.textSecondary, marginBottom: 10, ...Typography.default('semiBold') }}>
                                {showingSearch
                                    ? `${t('skills.searchResults')} (${hubSkills.length})`
                                    : t('skills.popular')
                                }
                            </Text>

                            {/* Loading */}
                            {(searchLoading || featuredLoading) && hubSkills.length === 0 ? (
                                <View style={{ paddingVertical: 30, alignItems: 'center' }}>
                                    <ActivityIndicator size="small" color={theme.colors.textSecondary} />
                                </View>
                            ) : hubSkills.length === 0 ? (
                                <View style={{ alignItems: 'center', paddingVertical: 30 }}>
                                    <Ionicons name="search-outline" size={32} color={theme.colors.textSecondary} />
                                    <Text style={{ color: theme.colors.textSecondary, fontSize: 14, marginTop: 8, ...Typography.default() }}>
                                        {showingSearch ? t('skills.noSearchResults') : t('skills.noSkills')}
                                    </Text>
                                </View>
                            ) : (
                                <View style={{ flexDirection: 'row', flexWrap: 'wrap', gap: 8 }}>
                                    {hubSkills.map(item => (
                                        <View key={item.slug} style={{ width: isNarrow ? '100%' : '48.5%' }}>
                                            <SkillHubCard
                                                item={item}
                                                onInstall={handleInstall}
                                                installing={installing}
                                                theme={theme}
                                            />
                                        </View>
                                    ))}
                                </View>
                            )}

                            {searchLoading && hubSkills.length > 0 && (
                                <View style={{ paddingVertical: 12, alignItems: 'center' }}>
                                    <ActivityIndicator size="small" color={theme.colors.textSecondary} />
                                </View>
                            )}
                        </View>
                    </>
                )}
            </ScrollView>

            <SkillDetailModal
                skill={detailSkill}
                visible={!!detailSkill}
                onClose={() => setDetailSkill(null)}
                onDisable={() => {
                    if (detailSkill) {
                        const isEnabled = enabledIds.has(detailSkill.id);
                        handleToggle(detailSkill.id, !isEnabled);
                    }
                }}
                onDelete={() => { if (detailSkill) handleDelete(detailSkill); }}
                enabled={detailSkill ? enabledIds.has(detailSkill.id) : false}
                theme={theme}
            />
        </View>
    );
}
