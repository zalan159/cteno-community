import React, { useMemo, useState } from 'react';
import { View, Modal, Pressable, TextInput, ActivityIndicator, ScrollView, Alert } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useUnistyles } from 'react-native-unistyles';
import { Typography } from '@/constants/Typography';
import { Text } from '@/components/StyledText';
import { frontendLog } from '@/utils/tauri';
import type { VendorName, WorkspaceRoleVendorOverrides, WorkspaceTemplateId } from '@/sync/ops';

type WorkspaceTemplateRole = {
    id: string;
    name: string;
    description: string;
    outputRoot: string;
    defaultVendor: VendorName;
};

type WorkspaceTemplateOption = {
    id: WorkspaceTemplateId;
    name: string;
    description: string;
    icon: keyof typeof Ionicons.glyphMap;
    roles: WorkspaceTemplateRole[];
};

const VENDOR_OPTIONS: VendorName[] = ['cteno', 'claude', 'codex', 'gemini'];

const VENDOR_LABEL: Record<VendorName, string> = {
    cteno: 'Cteno',
    claude: 'Claude',
    codex: 'Codex',
    gemini: 'Gemini',
};

const TEMPLATE_OPTIONS: WorkspaceTemplateOption[] = [
    // 群聊模板暂时下线
    // {
    //     id: 'group-chat',
    //     name: '群聊',
    //     description: '开放协作，多角色自由认领与公开协调',
    //     icon: 'chatbubbles-outline',
    //     roles: [
    //         {
    //             id: 'pm',
    //             name: 'PM',
    //             description: '负责范围澄清、分工和验收标准。',
    //             outputRoot: '00-management/',
    //             defaultVendor: 'cteno',
    //         },
    //         {
    //             id: 'prd',
    //             name: 'PRD',
    //             description: '把需求写成可执行的 PRD 与任务定义。',
    //             outputRoot: '10-prd/',
    //             defaultVendor: 'cteno',
    //         },
    //         {
    //             id: 'architect',
    //             name: 'Architect',
    //             description: '负责方案设计、接口与风险梳理。',
    //             outputRoot: '30-arch/',
    //             defaultVendor: 'cteno',
    //         },
    //         {
    //             id: 'coder',
    //             name: 'Coder',
    //             description: '实现代码改动并保持 diff 聚焦。',
    //             outputRoot: '40-code/',
    //             defaultVendor: 'cteno',
    //         },
    //         {
    //             id: 'tester',
    //             name: 'Tester',
    //             description: '执行验证、整理结果与回归风险。',
    //             outputRoot: '50-test/',
    //             defaultVendor: 'cteno',
    //         },
    //         {
    //             id: 'reviewer',
    //             name: 'Reviewer',
    //             description: '从缺陷、回归和遗漏测试角度审查结果。',
    //             outputRoot: '60-review/',
    //             defaultVendor: 'cteno',
    //         },
    //     ],
    // },
    {
        id: 'gated-tasks',
        name: '门控任务',
        description: '按任务账本逐项执行，评审通过后再推进',
        icon: 'git-network-outline',
        roles: [
            {
                id: 'reviewer',
                name: 'Reviewer',
                description: '基于任务账本逐项验收，实现通过前不允许提交。',
                outputRoot: '00-management/',
                defaultVendor: 'claude',
            },
            {
                id: 'coder',
                name: 'Coder',
                description: '一次只实现一个任务项，直到评审批准。',
                outputRoot: '40-code/',
                defaultVendor: 'codex',
            },
        ],
    },
    {
        id: 'autoresearch',
        name: '自主研究',
        description: '假设驱动的研究循环与证据沉淀',
        icon: 'search-outline',
        roles: [
            {
                id: 'lead',
                name: 'Lead',
                description: '定义研究问题、成功标准与下一轮方向。',
                outputRoot: 'research/00-lead/',
                defaultVendor: 'cteno',
            },
            {
                id: 'scout',
                name: 'Scout',
                description: '采集外部信号、引用与原始观察。',
                outputRoot: 'research/10-scout/',
                defaultVendor: 'cteno',
            },
            {
                id: 'experimenter',
                name: 'Experimenter',
                description: '把假设转成可执行、可衡量的实验。',
                outputRoot: 'research/20-experiments/',
                defaultVendor: 'cteno',
            },
            {
                id: 'critic',
                name: 'Critic',
                description: '质疑假设、暴露混杂因素并收紧推理。',
                outputRoot: 'research/30-critic/',
                defaultVendor: 'cteno',
            },
        ],
    },
];

function buildRoleVendorOverrides(template: WorkspaceTemplateOption): WorkspaceRoleVendorOverrides {
    return Object.fromEntries(template.roles.map((role) => [role.id, role.defaultVendor]));
}

interface CreateWorkspaceModalProps {
    visible: boolean;
    machineId?: string;
    workdir: string;
    onClose: () => void;
    onCreate: (params: {
        templateId: WorkspaceTemplateId;
        name: string;
        workdir: string;
        roleVendorOverrides: WorkspaceRoleVendorOverrides;
    }) => Promise<void>;
}

export const CreateWorkspaceModal: React.FC<CreateWorkspaceModalProps> = ({
    visible,
    workdir,
    onClose,
    onCreate,
}) => {
    const { theme } = useUnistyles();
    const defaultTemplateId = TEMPLATE_OPTIONS[0].id;
    const [templateId, setTemplateId] = useState<WorkspaceTemplateId>(defaultTemplateId);
    const [name, setName] = useState('');
    const [saving, setSaving] = useState(false);
    const [nameTouched, setNameTouched] = useState(false);
    const [roleVendorOverrides, setRoleVendorOverrides] = useState<WorkspaceRoleVendorOverrides>(() =>
        buildRoleVendorOverrides(TEMPLATE_OPTIONS[0])
    );

    const selectedTemplate = useMemo(
        () => TEMPLATE_OPTIONS.find((item) => item.id === templateId) || TEMPLATE_OPTIONS[0],
        [templateId]
    );

    const defaultWorkspaceName = useMemo(() => {
        switch (templateId) {
            case 'group-chat':
                return '群聊工作间';
            case 'gated-tasks':
                return '门控任务';
            case 'autoresearch':
                return '自主研究';
            default:
                return selectedTemplate.name;
        }
    }, [selectedTemplate.name, templateId]);

    React.useEffect(() => {
        if (!visible) return;
        setTemplateId(defaultTemplateId);
        setRoleVendorOverrides(buildRoleVendorOverrides(TEMPLATE_OPTIONS[0]));
        setNameTouched(false);
    }, [visible, defaultTemplateId]);

    React.useEffect(() => {
        if (!visible || nameTouched) return;
        setName(defaultWorkspaceName);
    }, [defaultWorkspaceName, nameTouched, visible]);

    const handleCreate = async () => {
        const trimmedName = name.trim() || defaultWorkspaceName;
        const resolvedRoleVendorOverrides = Object.fromEntries(
            selectedTemplate.roles.map((role) => [
                role.id,
                roleVendorOverrides[role.id] ?? role.defaultVendor,
            ])
        ) as WorkspaceRoleVendorOverrides;
        try {
            setSaving(true);
            frontendLog(`[CreateWorkspace] dispatching bootstrap-workspace template=${templateId} name=${trimmedName}`);
            await onCreate({
                templateId,
                name: trimmedName,
                workdir: workdir.trim() || '~',
                roleVendorOverrides: resolvedRoleVendorOverrides,
            });
            frontendLog(`[CreateWorkspace] bootstrap-workspace succeeded`);
            setTemplateId(defaultTemplateId);
            setRoleVendorOverrides(buildRoleVendorOverrides(TEMPLATE_OPTIONS[0]));
            setName('');
            setNameTouched(false);
            onClose();
        } catch (error) {
            const msg = error instanceof Error ? error.message : String(error);
            console.error('Failed to create workspace:', error);
            frontendLog(`[CreateWorkspace] bootstrap-workspace failed: ${msg}`, 'error');
            Alert.alert('创建工作间失败', msg);
        } finally {
            setSaving(false);
        }
    };

    return (
        <Modal visible={visible} transparent animationType="slide" onRequestClose={onClose}>
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
                        padding: 16,
                        gap: 16,
                    }}
                >
                    <View
                        style={{
                            flexDirection: 'row',
                            justifyContent: 'space-between',
                            alignItems: 'center',
                            borderBottomWidth: 1,
                            borderBottomColor: theme.colors.divider,
                            paddingBottom: 16,
                        }}
                    >
                        <Pressable onPress={onClose}>
                            <Text style={{ fontSize: 16, color: theme.colors.textSecondary, ...Typography.default() }}>
                                取消
                            </Text>
                        </Pressable>
                        <Text style={{ fontSize: 17, color: theme.colors.text, ...Typography.default('semiBold') }}>
                            新建工作间
                        </Text>
                        <Pressable onPress={handleCreate} disabled={saving}>
                            <Text
                                style={{
                                    fontSize: 16,
                                    color: !saving ? theme.colors.textLink : theme.colors.textSecondary,
                                    ...Typography.default('semiBold'),
                                }}
                            >
                                {saving ? '创建中' : '创建'}
                            </Text>
                        </Pressable>
                    </View>

                    <ScrollView
                        showsVerticalScrollIndicator={false}
                        contentContainerStyle={{ gap: 16, paddingBottom: 16 }}
                    >
                        <View style={{ gap: 8 }}>
                            <Text style={{ fontSize: 13, color: theme.colors.textSecondary, ...Typography.default('semiBold') }}>
                                模板
                            </Text>
                            <View style={{ gap: 8 }}>
                                {TEMPLATE_OPTIONS.map((item) => {
                                    const selected = item.id === templateId;
                                    return (
                                        <Pressable
                                            key={item.id}
                                            onPress={() => {
                                                setTemplateId(item.id);
                                                setRoleVendorOverrides(buildRoleVendorOverrides(item));
                                            }}
                                            style={({ pressed }) => ({
                                                borderRadius: 12,
                                                borderWidth: 1,
                                                borderColor: selected
                                                    ? theme.colors.button.primary.background
                                                    : theme.colors.divider,
                                                backgroundColor: pressed
                                                    ? theme.colors.surfacePressed
                                                    : selected
                                                        ? theme.colors.surfaceHigh
                                                        : theme.colors.surface,
                                                padding: 12,
                                                gap: 10,
                                            })}
                                        >
                                            <View style={{ flexDirection: 'row', alignItems: 'center' }}>
                                                <View
                                                    style={{
                                                        width: 36,
                                                        height: 36,
                                                        borderRadius: 18,
                                                        alignItems: 'center',
                                                        justifyContent: 'center',
                                                        backgroundColor: selected
                                                            ? theme.colors.button.primary.background
                                                            : theme.colors.surfaceHigh,
                                                        marginRight: 12,
                                                    }}
                                                >
                                                    <Ionicons
                                                        name={item.icon}
                                                        size={18}
                                                        color={selected ? theme.colors.button.primary.tint : theme.colors.text}
                                                    />
                                                </View>
                                                <View style={{ flex: 1 }}>
                                                    <Text style={{ fontSize: 15, color: theme.colors.text, ...Typography.default('semiBold') }}>
                                                        {item.name}
                                                    </Text>
                                                    <Text style={{ fontSize: 12, color: theme.colors.textSecondary, marginTop: 2, ...Typography.default() }}>
                                                        {item.id} · {item.description}
                                                    </Text>
                                                </View>
                                                {selected && (
                                                    <Ionicons
                                                        name="checkmark-circle"
                                                        size={18}
                                                        color={theme.colors.button.primary.background}
                                                    />
                                                )}
                                            </View>
                                            <View style={{ flexDirection: 'row', flexWrap: 'wrap', gap: 6 }}>
                                                {item.roles.map((role) => (
                                                    <View
                                                        key={role.id}
                                                        style={{
                                                            borderRadius: 999,
                                                            backgroundColor: theme.colors.surface,
                                                            borderWidth: 1,
                                                            borderColor: theme.colors.divider,
                                                            paddingHorizontal: 10,
                                                            paddingVertical: 6,
                                                        }}
                                                    >
                                                        <Text style={{ fontSize: 12, color: theme.colors.textSecondary, ...Typography.default() }}>
                                                            {role.name}
                                                        </Text>
                                                    </View>
                                                ))}
                                            </View>
                                        </Pressable>
                                    );
                                })}
                            </View>
                        </View>

                        <View style={{ gap: 8 }}>
                            <Text style={{ fontSize: 13, color: theme.colors.textSecondary, ...Typography.default('semiBold') }}>
                                角色与 Vendor
                            </Text>
                            <View style={{ gap: 10 }}>
                                {selectedTemplate.roles.map((role) => {
                                    const selectedVendor = roleVendorOverrides[role.id] ?? role.defaultVendor;
                                    return (
                                        <View
                                            key={role.id}
                                            style={{
                                                backgroundColor: theme.colors.surfaceHigh,
                                                borderRadius: 12,
                                                padding: 12,
                                                gap: 10,
                                            }}
                                        >
                                            <View style={{ gap: 4 }}>
                                                <View style={{ flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', gap: 8 }}>
                                                    <Text style={{ fontSize: 15, color: theme.colors.text, ...Typography.default('semiBold') }}>
                                                        {role.name}
                                                    </Text>
                                                    <Text style={{ fontSize: 11, color: theme.colors.textSecondary, ...Typography.default() }}>
                                                        {role.id} · {role.outputRoot}
                                                    </Text>
                                                </View>
                                                <Text style={{ fontSize: 12, color: theme.colors.textSecondary, ...Typography.default() }}>
                                                    {role.description}
                                                </Text>
                                            </View>
                                            <View style={{ flexDirection: 'row', gap: 8 }}>
                                                {VENDOR_OPTIONS.map((vendor) => {
                                                    const active = selectedVendor === vendor;
                                                    return (
                                                        <Pressable
                                                            key={vendor}
                                                            onPress={() =>
                                                                setRoleVendorOverrides((current) => ({
                                                                    ...current,
                                                                    [role.id]: vendor,
                                                                }))
                                                            }
                                                            style={({ pressed }) => ({
                                                                flex: 1,
                                                                borderRadius: 10,
                                                                borderWidth: 1,
                                                                borderColor: active
                                                                    ? theme.colors.button.primary.background
                                                                    : theme.colors.divider,
                                                                backgroundColor: pressed
                                                                    ? theme.colors.surfacePressed
                                                                    : active
                                                                        ? theme.colors.surface
                                                                        : theme.colors.surfaceHigh,
                                                                paddingVertical: 10,
                                                                alignItems: 'center',
                                                            })}
                                                        >
                                                            <Text
                                                                style={{
                                                                    fontSize: 13,
                                                                    color: active
                                                                        ? theme.colors.button.primary.background
                                                                        : theme.colors.textSecondary,
                                                                    ...Typography.default('semiBold'),
                                                                }}
                                                            >
                                                                {VENDOR_LABEL[vendor]}
                                                            </Text>
                                                        </Pressable>
                                                    );
                                                })}
                                            </View>
                                        </View>
                                    );
                                })}
                            </View>
                        </View>

                        <View style={{ gap: 8 }}>
                            <Text style={{ fontSize: 13, color: theme.colors.textSecondary, ...Typography.default('semiBold') }}>
                                工作间名称
                            </Text>
                            <TextInput
                                value={name}
                                onChangeText={(value) => {
                                    setName(value);
                                    setNameTouched(true);
                                }}
                                placeholder={defaultWorkspaceName}
                                placeholderTextColor={theme.colors.textSecondary}
                                style={{
                                    backgroundColor: theme.colors.surfaceHigh,
                                    borderRadius: 10,
                                    padding: 12,
                                    color: theme.colors.text,
                                    fontSize: 16,
                                    ...Typography.default(),
                                }}
                            />
                        </View>

                    </ScrollView>

                    {saving && (
                        <View style={{ flexDirection: 'row', alignItems: 'center', justifyContent: 'center', paddingBottom: 8 }}>
                            <ActivityIndicator size="small" color={theme.colors.textSecondary} />
                            <Text style={{ marginLeft: 8, color: theme.colors.textSecondary, ...Typography.default() }}>
                                正在创建工作间...
                            </Text>
                        </View>
                    )}
                </View>
            </View>
        </Modal>
    );
};
