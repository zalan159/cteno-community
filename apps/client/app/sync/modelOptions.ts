import type { PublicProxyModel } from './apiBalance';
import type { ModelOptionDisplay } from './ops';

const DEFAULT_COMPRESS_MODEL_ID = 'deepseek-chat';

function getCompressModelId(models: PublicProxyModel[]): string {
    return models.find(model => model.isCompressModel)?.id || DEFAULT_COMPRESS_MODEL_ID;
}

function normalizeApiFormat(
    apiFormat: PublicProxyModel['apiFormat']
): ModelOptionDisplay['apiFormat'] {
    if (apiFormat === 'openai' || apiFormat === 'gemini') {
        return apiFormat;
    }
    return 'anthropic';
}

export function buildModelOptionFromProxyModel(
    model: PublicProxyModel,
    compressModelId: string
): ModelOptionDisplay {
    return {
        id: `proxy-${model.id}`,
        name: model.name,
        isProxy: true,
        isFree: model.isFree === true,
        supportsVision: model.supportsVision === true,
        supportsComputerUse: model.supportsComputerUse === true,
        apiFormat: normalizeApiFormat(model.apiFormat),
        chat: {
            api_key_masked: '',
            base_url: '',
            model: model.id,
            temperature: model.temperature ?? 0.7,
            max_tokens: 32000,
            context_window_tokens: model.contextWindowTokens,
        },
        compress: {
            api_key_masked: '',
            base_url: '',
            model: compressModelId,
            temperature: 0.3,
            max_tokens: 3200,
        },
    };
}

export function mergeModelsWithServerProxyModels(
    models: ModelOptionDisplay[],
    proxyModels: PublicProxyModel[]
): ModelOptionDisplay[] {
    if (proxyModels.length === 0) {
        return models;
    }

    const compressModelId = getCompressModelId(proxyModels);
    const existingById = new Map(models.map(model => [model.id, model]));
    const mergedProxyModels = proxyModels.map((proxyModel) => {
        const serverModel = buildModelOptionFromProxyModel(proxyModel, compressModelId);
        const existingModel = existingById.get(serverModel.id);

        if (!existingModel) {
            return serverModel;
        }

        return {
            ...existingModel,
            ...serverModel,
            chat: {
                ...existingModel.chat,
                ...serverModel.chat,
            },
            compress: {
                ...existingModel.compress,
                ...serverModel.compress,
            },
        };
    });

    const customModels = models.filter(model => !model.isProxy);
    return [...mergedProxyModels, ...customModels];
}
