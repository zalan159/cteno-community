import type { TranslationStructure } from '../_default';
import { en } from './en';

function isPlainObject(value: unknown): value is Record<string, unknown> {
    return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function deepMerge(base: TranslationStructure, override: TranslationStructure): TranslationStructure {
    const result: Record<string, unknown> = { ...(base as Record<string, unknown>) };

    for (const [key, overrideValue] of Object.entries(override as Record<string, unknown>)) {
        const baseValue = (base as Record<string, unknown>)[key];

        if (isPlainObject(baseValue) && isPlainObject(overrideValue)) {
            result[key] = deepMerge(baseValue as TranslationStructure, overrideValue as TranslationStructure);
        } else {
            result[key] = overrideValue;
        }
    }

    return result as TranslationStructure;
}

export function withEnglishFallback(locale: TranslationStructure): TranslationStructure {
    return deepMerge(en as TranslationStructure, locale);
}
