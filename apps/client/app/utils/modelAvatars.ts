import {
    AVATAR_DEEPSEEK,
    AVATAR_KIMI,
    AVATAR_ZHIPU,
    AVATAR_MINIMAX,
    AVATAR_QWEN,
    AVATAR_ANTHROPIC,
    AVATAR_VOLCENGINE,
    AVATAR_GEMINI,
    AVATAR_OPENAI,
    AVATAR_STEPFUN,
    AVATAR_NVIDIA,
} from './modelAvatarData';

// Model avatar data URIs keyed by avatarId
// Using base64 data URIs so they're embedded in the JS bundle,
// avoiding OTA asset-stripping issues.
export const MODEL_AVATAR_IMAGES: Record<string, string> = {
    deepseek: AVATAR_DEEPSEEK,
    kimi: AVATAR_KIMI,
    zhipu: AVATAR_ZHIPU,
    minimax: AVATAR_MINIMAX,
    qwen: AVATAR_QWEN,
    anthropic: AVATAR_ANTHROPIC,
    volcengine: AVATAR_VOLCENGINE,
    gemini: AVATAR_GEMINI,
    openai: AVATAR_OPENAI,
    stepfun: AVATAR_STEPFUN,
    nvidia: AVATAR_NVIDIA,
};

// Check if avatarId corresponds to a model PNG avatar
export function isModelAvatar(avatarId: string): boolean {
    return avatarId in MODEL_AVATAR_IMAGES;
}

// Map modelId to a default model avatarId
// e.g. "proxy-deepseek-reasoner" → "deepseek"
export function getDefaultAvatarForModelId(modelId: string): string | null {
    const id = modelId.replace(/^proxy-/, '').toLowerCase();

    if (id.includes('deepseek')) return 'deepseek';
    if (id.includes('kimi') || id.includes('moonshot')) return 'kimi';
    if (id.includes('glm') || id.includes('zhipu') || id.includes('z-ai')) return 'zhipu';
    if (id.includes('minimax')) return 'minimax';
    if (id.includes('qwen') || id.includes('bailian')) return 'qwen';
    if (id.includes('anthropic') || id.includes('claude')) return 'anthropic';
    if (id.includes('volcengine') || id.includes('doubao')) return 'volcengine';
    if (id.includes('gemini') || id.includes('google')) return 'gemini';
    if (id.includes('openai') || id.includes('gpt')) return 'openai';
    if (id.includes('stepfun') || id.includes('step-')) return 'stepfun';
    if (id.includes('nvidia') || id.includes('nemotron')) return 'nvidia';

    return null;
}

export function getDefaultAvatarForModelOption(params: {
    modelId?: string | null;
    vendor?: string | null;
}): string | null {
    const modelAvatar = params.modelId ? getDefaultAvatarForModelId(params.modelId) : null;
    if (modelAvatar) {
        return modelAvatar;
    }

    switch ((params.vendor || '').toLowerCase()) {
        case 'claude':
            return 'anthropic';
        case 'codex':
            return 'openai';
        case 'gemini':
            return 'gemini';
        case 'cteno':
            return 'minimax';
        default:
            return null;
    }
}

// Get model badge image URI for a selected model.
// Returns the logo data URI if modelId maps to a known provider, null otherwise.
export function getModelBadgeUri(modelId: string | null | undefined): string | null {
    if (!modelId) return null;
    const avatarKey = getDefaultAvatarForModelId(modelId);
    if (!avatarKey) return null;
    return MODEL_AVATAR_IMAGES[avatarKey] ?? null;
}
