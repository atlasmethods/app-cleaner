import en from './en.json';

/** Translation catalogs by language code. Add a language = add a JSON file with the same keys here. */
export const CATALOGS: Record<string, Record<string, string>> = { en };

/** Languages offered in Settings (every key of CATALOGS). */
export const LANGUAGES: { code: string; label: string }[] = [{ code: 'en', label: 'English' }];

export type MessageKey = keyof typeof en;

let current = 'en';

/** Switch the UI language; unknown codes fall back to English. */
export function setLanguage(code: string): void {
  current = code in CATALOGS ? code : 'en';
}

export function getLanguage(): string {
  return current;
}

/** The text for `key` in the current language, then English, then the key itself. */
export function t(key: MessageKey | (string & {})): string {
  return CATALOGS[current]?.[key] ?? CATALOGS.en?.[key] ?? key;
}
