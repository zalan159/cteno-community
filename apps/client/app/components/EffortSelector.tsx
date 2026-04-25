import * as React from 'react';
import { Ionicons } from '@expo/vector-icons';
import { ItemGroup } from '@/components/ItemGroup';
import { Item } from '@/components/Item';
import { useUnistyles } from 'react-native-unistyles';

export type RuntimeEffort = 'default' | 'low' | 'medium' | 'high' | 'xhigh' | 'max';

interface EffortSelectorProps {
    value: RuntimeEffort;
    onChange: (value: RuntimeEffort) => void;
    title?: string;
    availableLevels?: RuntimeEffort[];
}

const EFFORT_OPTIONS: Array<{
    value: RuntimeEffort;
    label: string;
    description: string;
    icon: React.ComponentProps<typeof Ionicons>['name'];
}> = [
    {
        value: 'default',
        label: '默认',
        description: '跟随模型或运行时默认推理强度',
        icon: 'sparkles-outline',
    },
    {
        value: 'low',
        label: '低',
        description: '更快响应，减少额外思考',
        icon: 'flash-outline',
    },
    {
        value: 'medium',
        label: '中',
        description: '平衡速度与推理质量',
        icon: 'speedometer-outline',
    },
    {
        value: 'high',
        label: '高',
        description: '优先更深的推理与规划',
        icon: 'flask-outline',
    },
    {
        value: 'xhigh',
        label: '超高',
        description: '更强的规划与推理，响应通常更慢',
        icon: 'rocket-outline',
    },
    {
        value: 'max',
        label: '最大',
        description: '使用模型允许的最高推理强度',
        icon: 'diamond-outline',
    },
];

export function EffortSelector({
    value,
    onChange,
    title = '推理强度',
    availableLevels,
}: EffortSelectorProps) {
    const { theme } = useUnistyles();
    const options = React.useMemo(() => {
        if (!availableLevels || availableLevels.length === 0) {
            return EFFORT_OPTIONS;
        }
        const allowed = new Set<RuntimeEffort>(availableLevels.includes('default')
            ? availableLevels
            : ['default', ...availableLevels]);
        return EFFORT_OPTIONS.filter((option) => allowed.has(option.value));
    }, [availableLevels]);

    return (
        <ItemGroup title={title}>
            {options.map((option, index) => (
                <Item
                    key={option.value}
                    title={option.label}
                    subtitle={option.description}
                    leftElement={
                        <Ionicons
                            name={option.icon}
                            size={24}
                            color={value === option.value ? theme.colors.button.primary.tint : theme.colors.textSecondary}
                        />
                    }
                    rightElement={value === option.value ? (
                        <Ionicons
                            name="checkmark-circle"
                            size={20}
                            color={theme.colors.button.primary.tint}
                        />
                    ) : null}
                    onPress={() => onChange(option.value)}
                    showChevron={false}
                    selected={value === option.value}
                    showDivider={index < options.length - 1}
                />
            ))}
        </ItemGroup>
    );
}
