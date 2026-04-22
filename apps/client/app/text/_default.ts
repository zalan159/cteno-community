import { en as englishTranslations } from './translations/en';

export const en = englishTranslations;

export type Translations = typeof englishTranslations;

type TranslationLeaf = string | ((...args: any[]) => string);

// TranslationStructure keeps English keys as a template, but allows
// partial/non-strict locale files during translation rollout.
export type TranslationStructure<T = Translations> = T extends TranslationLeaf
  ? T extends string
    ? string
    : T
  : T extends object
    ? ({
        readonly [K in keyof T]?: TranslationStructure<T[K]>;
      } & {
        readonly [key: string]: TranslationStructure<any> | TranslationLeaf | undefined;
      })
    : T;
