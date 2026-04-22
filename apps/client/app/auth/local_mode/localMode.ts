import { isTauri } from '@/utils/tauri';

export function isDesktopLocalModeEnabled(): boolean {
    return isTauri();
}
