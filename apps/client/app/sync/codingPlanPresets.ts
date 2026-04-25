import type { LlmProfileInput } from './ops';

export type CodingPlanProviderId = 'glm' | 'kimi' | 'minimax' | 'bailian';
export type CodingPlanRegion = 'intl' | 'china';

export interface CodingPlanPresetModel {
    id: string;
    name: string;
    contextWindowTokens: number;
    thinking?: boolean;
    supportsVision?: boolean;
}

export interface CodingPlanPresetProvider {
    id: CodingPlanProviderId;
    name: string;
    description: string;
    defaultRegion: CodingPlanRegion;
    regions: Array<{
        id: CodingPlanRegion;
        label: string;
        baseUrl: string;
    }>;
    defaultModelId: string;
    compressModelId: string;
    models: CodingPlanPresetModel[];
}

const MAX_TOKENS = 32000;
const COMPRESS_MAX_TOKENS = 4096;

export const CODING_PLAN_PRESETS: CodingPlanPresetProvider[] = [
    {
        id: 'glm',
        name: 'GLM Coding Plan',
        description: '智谱 Coding Plan，适合 GLM 5.x/4.x coding 模型。',
        defaultRegion: 'china',
        regions: [
            {
                id: 'china',
                label: '中国区',
                baseUrl: 'https://api.z.ai/api/anthropic',
            },
        ],
        defaultModelId: 'GLM-5.1',
        compressModelId: 'GLM-4.5-Air',
        models: [
            { id: 'GLM-5.1', name: 'GLM-5.1', contextWindowTokens: 200000, thinking: true },
            { id: 'GLM-5-Turbo', name: 'GLM-5 Turbo', contextWindowTokens: 200000, thinking: true },
            { id: 'GLM-4.7', name: 'GLM-4.7', contextWindowTokens: 200000, thinking: true },
            { id: 'GLM-4.5-Air', name: 'GLM-4.5 Air', contextWindowTokens: 128000 },
        ],
    },
    {
        id: 'kimi',
        name: 'Kimi Code',
        description: 'Moonshot Kimi Code / K2.5 coding 专用入口。',
        defaultRegion: 'intl',
        regions: [
            {
                id: 'intl',
                label: '国际区',
                baseUrl: 'https://api.kimi.com/coding',
            },
        ],
        defaultModelId: 'kimi-for-coding',
        compressModelId: 'kimi-for-coding',
        models: [
            { id: 'kimi-for-coding', name: 'Kimi For Coding', contextWindowTokens: 262000, thinking: true, supportsVision: true },
        ],
    },
    {
        id: 'minimax',
        name: 'MiniMax Token Plan',
        description: 'MiniMax Coding/Token Plan，支持 M2.7 与 M2.5。',
        defaultRegion: 'intl',
        regions: [
            {
                id: 'intl',
                label: '国际区',
                baseUrl: 'https://api.minimax.io/anthropic',
            },
            {
                id: 'china',
                label: '中国区',
                baseUrl: 'https://api.minimaxi.com/anthropic',
            },
        ],
        defaultModelId: 'MiniMax-M2.7',
        compressModelId: 'MiniMax-M2.5',
        models: [
            { id: 'MiniMax-M2.7', name: 'MiniMax M2.7', contextWindowTokens: 200000, thinking: true },
            { id: 'MiniMax-M2.7-highspeed', name: 'MiniMax M2.7 Highspeed', contextWindowTokens: 200000, thinking: true },
            { id: 'MiniMax-M2.5', name: 'MiniMax M2.5', contextWindowTokens: 200000 },
        ],
    },
    {
        id: 'bailian',
        name: '百炼 Coding Plan',
        description: '阿里云百炼 Coding Plan，一个 SK 覆盖 Qwen/Kimi/GLM/MiniMax 代码模型。',
        defaultRegion: 'intl',
        regions: [
            {
                id: 'intl',
                label: '国际区',
                baseUrl: 'https://coding-intl.dashscope.aliyuncs.com/apps/anthropic',
            },
            {
                id: 'china',
                label: '中国区',
                baseUrl: 'https://coding.dashscope.aliyuncs.com/apps/anthropic',
            },
        ],
        defaultModelId: 'qwen3.5-plus',
        compressModelId: 'qwen3.5-plus',
        models: [
            { id: 'qwen3.5-plus', name: 'Qwen 3.5 Plus', contextWindowTokens: 1000000, thinking: true, supportsVision: true },
            { id: 'kimi-k2.5', name: 'Kimi K2.5', contextWindowTokens: 262000, thinking: true, supportsVision: true },
            { id: 'glm-5', name: 'GLM-5', contextWindowTokens: 200000, thinking: true },
            { id: 'MiniMax-M2.5', name: 'MiniMax M2.5', contextWindowTokens: 200000 },
            { id: 'qwen3-max-2026-01-23', name: 'Qwen 3 Max', contextWindowTokens: 1000000, thinking: true, supportsVision: true },
            { id: 'qwen3-coder-next', name: 'Qwen 3 Coder Next', contextWindowTokens: 1000000, thinking: true },
            { id: 'qwen3-coder-plus', name: 'Qwen 3 Coder Plus', contextWindowTokens: 1000000, thinking: true },
            { id: 'glm-4.7', name: 'GLM-4.7', contextWindowTokens: 200000, thinking: true },
        ],
    },
];

export function getCodingPlanProvider(id: CodingPlanProviderId): CodingPlanPresetProvider {
    return CODING_PLAN_PRESETS.find(provider => provider.id === id) || CODING_PLAN_PRESETS[0];
}

export function buildCodingPlanProfiles(
    provider: CodingPlanPresetProvider,
    region: CodingPlanRegion,
    apiKey: string
): { profiles: LlmProfileInput[]; defaultProfileId: string } {
    const regionConfig =
        provider.regions.find(item => item.id === region) ||
        provider.regions.find(item => item.id === provider.defaultRegion) ||
        provider.regions[0];
    const baseUrl = regionConfig.baseUrl;
    const compressModel =
        provider.models.find(model => model.id === provider.compressModelId) ||
        provider.models.find(model => model.id === provider.defaultModelId) ||
        provider.models[0];

    const profiles = provider.models.map((model) => ({
        id: `coding-plan-${provider.id}-${model.id}`.replace(/[^a-zA-Z0-9_.-]/g, '-'),
        name: `${provider.name} · ${model.name}`,
        chat: {
            api_key: apiKey,
            base_url: baseUrl,
            model: model.id,
            temperature: model.thinking ? 1 : 0.7,
            max_tokens: MAX_TOKENS,
            context_window_tokens: model.contextWindowTokens,
        },
        compress: {
            api_key: apiKey,
            base_url: baseUrl,
            model: compressModel.id,
            temperature: 0.3,
            max_tokens: COMPRESS_MAX_TOKENS,
            context_window_tokens: compressModel.contextWindowTokens,
        },
        supports_vision: model.supportsVision === true,
        supports_computer_use: false,
        supports_function_calling: true,
        supports_image_output: false,
        api_format: 'anthropic' as const,
        thinking: model.thinking === true,
    }));

    const defaultProfile = profiles.find(profile => profile.chat.model === provider.defaultModelId) || profiles[0];
    return {
        profiles,
        defaultProfileId: defaultProfile.id,
    };
}
