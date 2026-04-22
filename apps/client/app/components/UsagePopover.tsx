import * as React from 'react';
import { Platform, Pressable, TouchableWithoutFeedback, View } from 'react-native';
import { Text } from '@/components/StyledText';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Typography } from '@/constants/Typography';
import { FloatingOverlay } from './FloatingOverlay';
import {
    formatRemainingPercent,
    formatResetCountdown,
    labelForWindowKey,
    pickPrimaryUsage,
    remainingColor,
    useVendorUsage,
} from '@/utils/useVendorUsage';
import type { VendorUsage, VendorUsageId } from '@/sync/storageTypes';
import { storage } from '@/sync/storage';

const stylesheet = StyleSheet.create((theme) => ({
    wrapper: {
        position: 'relative',
    },
    trigger: {
        flexDirection: 'row',
        alignItems: 'center',
    },
    triggerText: {
        fontSize: 11,
        marginLeft: 8,
        ...Typography.default(),
    },
    overlay: {
        position: 'absolute',
        bottom: '100%',
        left: 0,
        // Detach from the tiny trigger's width so the popover can grow to
        // fit row content. `minWidth` keeps it comfortably readable; the
        // overlay will extend past the trigger's right edge into empty row
        // space to the right (the status line is left-aligned).
        minWidth: 280,
        marginBottom: 8,
        zIndex: 1000,
        borderRadius: 12,
        // FloatingOverlay drops its border on web (Tauri is web); add a
        // visible frame so the popover doesn't blend into the chat surface.
        borderWidth: Platform.OS === 'web' ? 1 : 0.5,
        borderColor: theme.colors.divider,
        overflow: 'hidden',
    },
    backdrop: {
        position: 'absolute',
        top: -1000,
        left: -1000,
        right: -1000,
        bottom: -1000,
        zIndex: 999,
    },
    sectionTitle: {
        fontSize: 12,
        fontWeight: '600',
        color: theme.colors.textSecondary,
        paddingHorizontal: 16,
        paddingTop: 8,
        paddingBottom: 4,
        ...Typography.default('semiBold'),
    },
    row: {
        flexDirection: 'row',
        alignItems: 'center',
        justifyContent: 'space-between',
        paddingHorizontal: 16,
        paddingVertical: 6,
        gap: 16,
    },
    rowLeft: {
        flexShrink: 1,
    },
    rowLabel: {
        fontSize: 14,
        color: theme.colors.text,
        ...Typography.default(),
    },
    rowValue: {
        fontSize: 14,
        fontWeight: '600',
        ...Typography.default('semiBold'),
    },
    rowReset: {
        fontSize: 11,
        color: theme.colors.textSecondary,
        marginTop: 2,
        ...Typography.default(),
    },
    planMeta: {
        paddingHorizontal: 16,
        paddingVertical: 8,
        fontSize: 11,
        color: theme.colors.textSecondary,
        ...Typography.default(),
    },
    errorRow: {
        paddingHorizontal: 16,
        paddingVertical: 8,
        fontSize: 12,
        color: theme.colors.textDestructive,
        ...Typography.default(),
    },
}));

interface Props {
    machineId: string | null | undefined;
    vendor: VendorUsageId | null | undefined;
    preferredModelId?: string | null;
}

export const UsagePopover = React.memo(({ machineId, vendor, preferredModelId }: Props) => {
    const styles = stylesheet;
    const { theme } = useUnistyles();
    const [open, setOpen] = React.useState(false);

    // Fallback: if the session doesn't carry a machineId (common for
    // freshly-spawned local sessions before metadata round-trips), pick
    // the only registered machine. Usage is machine-scoped but in practice
    // one daemon → one machine, so this is unambiguous.
    const fallbackMachineId = storage((s) => {
        if (machineId) return null;
        const ids = Object.keys(s.machines);
        return ids.length > 0 ? ids[0] : null;
    });
    const effectiveMachineId = machineId ?? fallbackMachineId;

    const usage = useVendorUsage(effectiveMachineId, vendor);

    // Re-render every 30s so "x min 后重置" stays fresh without a Vue-style
    // ticking timer per row.
    const [, setTick] = React.useState(0);
    React.useEffect(() => {
        const id = setInterval(() => setTick((n) => n + 1), 30_000);
        return () => clearInterval(id);
    }, []);

    if (!vendor || !effectiveMachineId) return null;
    const primary = pickPrimaryUsage(usage, preferredModelId);

    // Nothing to show yet (cold start, probe hasn't run): render a subtle
    // placeholder instead of flashing different widths each poll.
    const triggerLabel = (() => {
        if (usage?.error) return `• usage: 不可用`;
        if (!primary) return `• usage: …`;
        const pct = formatRemainingPercent(primary.usedPercent);
        return `• 剩余 ${pct} · ${primary.label}`;
    })();

    const triggerColor = primary ? remainingColor(primary.usedPercent) : theme.colors.textSecondary;

    return (
        <View style={styles.wrapper}>
            <Pressable onPress={() => setOpen((v) => !v)} style={styles.trigger} hitSlop={6}>
                <Text style={[styles.triggerText, { color: triggerColor }]}>{triggerLabel}</Text>
            </Pressable>
            {open && (
                <>
                    <TouchableWithoutFeedback onPress={() => setOpen(false)}>
                        <View style={styles.backdrop} />
                    </TouchableWithoutFeedback>
                    <View style={styles.overlay}>
                        <FloatingOverlay maxHeight={420} keyboardShouldPersistTaps="always">
                            <UsageDetails usage={usage} />
                        </FloatingOverlay>
                    </View>
                </>
            )}
        </View>
    );
});

function UsageDetails({ usage }: { usage: VendorUsage | null }) {
    const styles = stylesheet;
    const { theme } = useUnistyles();

    if (!usage) {
        return <Text style={styles.planMeta}>正在获取用量…</Text>;
    }
    if (usage.error) {
        return <Text style={styles.errorRow}>{usage.error}</Text>;
    }

    const rows: React.ReactNode[] = [];
    if (usage.shape === 'windows') {
        rows.push(
            <Text key="title" style={styles.sectionTitle}>{vendorTitle(usage.provider)}</Text>,
        );
        const order = ['fiveHour', 'weekly', 'weeklyOpus', 'weeklySonnet', 'overage'];
        const keysInOrder = [
            ...order.filter((k) => usage.windows?.[k]),
            ...Object.keys(usage.windows ?? {}).filter((k) => !order.includes(k)),
        ];
        if (keysInOrder.length === 0) {
            rows.push(<Text key="empty" style={styles.planMeta}>暂无窗口数据</Text>);
        }
        for (const key of keysInOrder) {
            const w = usage.windows?.[key];
            if (!w) continue;
            rows.push(
                <View key={key} style={styles.row}>
                    <View style={styles.rowLeft}>
                        <Text style={styles.rowLabel}>{labelForWindowKey(key)}</Text>
                        <Text style={styles.rowReset} numberOfLines={1}>{formatResetCountdown(w.resetsAt)}</Text>
                    </View>
                    <Text style={[styles.rowValue, { color: remainingColor(w.usedPercent) }]}>
                        剩余 {formatRemainingPercent(w.usedPercent)}
                    </Text>
                </View>,
            );
        }
    } else {
        rows.push(
            <Text key="title" style={styles.sectionTitle}>{vendorTitle(usage.provider)} · 模型限额</Text>,
        );
        if ((usage.buckets ?? []).length === 0) {
            rows.push(<Text key="empty" style={styles.planMeta}>暂无桶数据</Text>);
        }
        for (const bucket of usage.buckets ?? []) {
            rows.push(
                <View key={bucket.modelId} style={styles.row}>
                    <View style={styles.rowLeft}>
                        <Text style={styles.rowLabel} numberOfLines={1}>{bucket.modelId}</Text>
                        <Text style={styles.rowReset} numberOfLines={1}>
                            {bucket.tokenType.toLowerCase()} · {formatResetCountdown(bucket.resetsAt)}
                        </Text>
                    </View>
                    <Text style={[styles.rowValue, { color: remainingColor(bucket.usedPercent) }]}>
                        剩余 {formatRemainingPercent(bucket.usedPercent)}
                    </Text>
                </View>,
            );
        }
    }

    if (usage.planType) {
        rows.push(
            <Text key="plan" style={styles.planMeta}>
                计划：{usage.planType}
                {usage.credits?.balance ? `  ·  余额 ${usage.credits.balance}` : ''}
            </Text>,
        );
    }

    return <>{rows}</>;
}

function vendorTitle(v: 'claude' | 'codex' | 'gemini'): string {
    switch (v) {
        case 'claude': return 'Claude 订阅额度';
        case 'codex': return 'Codex (ChatGPT) 额度';
        case 'gemini': return 'Gemini 额度';
    }
}
