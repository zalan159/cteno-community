import { describe, expect, it } from 'vitest';
import type { ModelOptionDisplay } from './ops';
import {
    buildModelOptionFromProxyModel,
    filterProxyModelsForAuth,
    mergeModelsWithServerProxyModels,
} from './modelOptions';

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
            thinking: true,
        }, 'deepseek-chat');

        expect(option).toEqual({
            id: 'proxy-gpt-5.4',
            name: 'GPT-5.4',
            isProxy: true,
            isFree: true,
            supportsVision: true,
            supportsComputerUse: true,
            apiFormat: 'openai',
            thinking: true,
            supportedReasoningEfforts: ['default', 'high', 'max'],
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
                id: 'deepseek-v4-flash',
                name: 'DeepSeek V4 Flash',
                inputRate: 1,
                outputRate: 2,
                isCompressModel: true,
                apiFormat: 'anthropic',
                thinking: true,
            },
        ]);

        expect(merged.map(model => model.id)).toEqual([
            'proxy-glm-4.7-flash',
            'proxy-deepseek-v4-flash',
            'user-openai',
        ]);
        expect(merged[0].name).toBe('GLM-4.7 Flash');
        expect(merged[0].isFree).toBe(true);
        expect(merged[0].compress.model).toBe('deepseek-v4-flash');
        expect(merged[1].thinking).toBe(true);
        expect(merged[1].supportedReasoningEfforts).toEqual(['default', 'high', 'max']);
        expect(merged[2].name).toBe('My OpenAI');
    });

    it('removes cached proxy options when the account cannot use cloud proxy access', () => {
        const models: ModelOptionDisplay[] = [
            {
                id: 'proxy-deepseek-v4-flash',
                name: 'DeepSeek Proxy',
                isProxy: true,
                chat: {
                    api_key_masked: '',
                    base_url: '',
                    model: 'deepseek-v4-flash',
                    temperature: 0.7,
                    max_tokens: 32000,
                },
                compress: {
                    api_key_masked: '',
                    base_url: '',
                    model: 'deepseek-v4-flash',
                    temperature: 0.3,
                    max_tokens: 3200,
                },
            },
            {
                id: 'local-openai',
                name: 'Local OpenAI',
                isProxy: false,
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

        expect(filterProxyModelsForAuth(models, false).map(model => model.id)).toEqual(['local-openai']);
        expect(filterProxyModelsForAuth(models, true).map(model => model.id)).toEqual([
            'proxy-deepseek-v4-flash',
            'local-openai',
        ]);
    });
});
