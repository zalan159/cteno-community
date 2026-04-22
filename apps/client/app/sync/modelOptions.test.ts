import { describe, expect, it } from 'vitest';
import type { ModelOptionDisplay } from './ops';
import { buildModelOptionFromProxyModel, mergeModelsWithServerProxyModels } from './modelOptions';

describe('modelOptions', () => {
    it('builds a model option from a public proxy model', () => {
        const option = buildModelOptionFromProxyModel({
            id: 'gpt-5.4',
            name: 'GPT-5.4',
            inputRate: 1,
            outputRate: 2,
            contextWindowTokens: 256000,
            supportsVision: true,
            supportsComputerUse: true,
            isFree: true,
            apiFormat: 'openai',
            temperature: 1,
        }, 'deepseek-chat');

        expect(option).toEqual({
            id: 'proxy-gpt-5.4',
            name: 'GPT-5.4',
            isProxy: true,
            isFree: true,
            supportsVision: true,
            supportsComputerUse: true,
            apiFormat: 'openai',
            chat: {
                api_key_masked: '',
                base_url: '',
                model: 'gpt-5.4',
                temperature: 1,
                max_tokens: 32000,
                context_window_tokens: 256000,
            },
            compress: {
                api_key_masked: '',
                base_url: '',
                model: 'deepseek-chat',
                temperature: 0.3,
                max_tokens: 3200,
            },
        });
    });

    it('replaces proxy options with server models while preserving custom models', () => {
        const existingModels: ModelOptionDisplay[] = [
            {
                id: 'proxy-old-model',
                name: 'Old Proxy Model',
                isProxy: true,
                chat: {
                    api_key_masked: '',
                    base_url: '',
                    model: 'old-model',
                    temperature: 0.7,
                    max_tokens: 32000,
                },
                compress: {
                    api_key_masked: '',
                    base_url: '',
                    model: 'deepseek-chat',
                    temperature: 0.3,
                    max_tokens: 3200,
                },
            },
            {
                id: 'proxy-glm-4.7-flash',
                name: 'Stale GLM Name',
                isProxy: true,
                chat: {
                    api_key_masked: '',
                    base_url: '',
                    model: 'glm-4.7-flash',
                    temperature: 0.7,
                    max_tokens: 32000,
                },
                compress: {
                    api_key_masked: '',
                    base_url: '',
                    model: 'old-compress',
                    temperature: 0.3,
                    max_tokens: 3200,
                },
            },
            {
                id: 'user-openai',
                name: 'My OpenAI',
                chat: {
                    api_key_masked: 'sk-***',
                    base_url: 'https://api.openai.com/v1',
                    model: 'gpt-5.4-mini',
                    temperature: 1,
                    max_tokens: 16000,
                },
                compress: {
                    api_key_masked: 'sk-***',
                    base_url: 'https://api.openai.com/v1',
                    model: 'gpt-5.4-mini',
                    temperature: 0.3,
                    max_tokens: 4000,
                },
            },
        ];

        const merged = mergeModelsWithServerProxyModels(existingModels, [
            {
                id: 'glm-4.7-flash',
                name: 'GLM-4.7 Flash',
                inputRate: 0,
                outputRate: 0,
                isFree: true,
                apiFormat: 'anthropic',
            },
            {
                id: 'deepseek-chat',
                name: 'DeepSeek Chat',
                inputRate: 1,
                outputRate: 2,
                isCompressModel: true,
                apiFormat: 'anthropic',
            },
        ]);

        expect(merged.map(model => model.id)).toEqual([
            'proxy-glm-4.7-flash',
            'proxy-deepseek-chat',
            'user-openai',
        ]);
        expect(merged[0].name).toBe('GLM-4.7 Flash');
        expect(merged[0].isFree).toBe(true);
        expect(merged[0].compress.model).toBe('deepseek-chat');
        expect(merged[2].name).toBe('My OpenAI');
    });
});
