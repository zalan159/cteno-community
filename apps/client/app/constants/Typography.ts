import { Platform } from 'react-native';

/**
 * Typography system for Cteno app
 * 
 * Default typography: IBM Plex Sans
 * Monospace typography: IBM Plex Mono  
 * Logo typography: Bricolage Grotesque (specific use only)
 * 
 * Usage Examples:
 * 
 * // Default typography (IBM Plex Sans)
 * <Text style={{ fontSize: 16, ...Typography.default() }}>Regular text</Text>
 * <Text style={{ fontSize: 16, ...Typography.default('italic') }}>Italic text</Text>
 * <Text style={{ fontSize: 16, ...Typography.default('semiBold') }}>Semi-bold text</Text>
 * 
 * // Monospace typography (IBM Plex Mono)
 * <Text style={{ fontSize: 14, ...Typography.mono() }}>Code text</Text>
 * <Text style={{ fontSize: 14, ...Typography.mono('italic') }}>Italic code</Text>
 * <Text style={{ fontSize: 14, ...Typography.mono('semiBold') }}>Bold code</Text>
 * 
 * // Logo typography (Bricolage Grotesque - use sparingly!)
 * // Note: Don't add fontWeight as this font is already bold
 * <Text style={{ fontSize: 28, ...Typography.logo() }}>Logo Text</Text>
 * 
 * // Alternative direct usage
 * <Text style={{ fontSize: 16, fontFamily: getDefaultFont('semiBold') }}>Direct usage</Text>
 * <Text style={{ fontSize: 14, fontFamily: getMonoFont() }}>Direct mono usage</Text>
 * <Text style={{ fontSize: 28, fontFamily: getLogoFont() }}>Direct logo usage</Text>
 */

// Font family constants
export const FontFamilies = {
  // IBM Plex Sans (default typography)
  default: {
    regular: 'IBMPlexSans-Regular',
    italic: 'IBMPlexSans-Italic', 
    semiBold: 'IBMPlexSans-SemiBold',
  },
  
  // IBM Plex Mono (default monospace)
  mono: {
    regular: 'IBMPlexMono-Regular',
    italic: 'IBMPlexMono-Italic',
    semiBold: 'IBMPlexMono-SemiBold',
  },
  
  // Bricolage Grotesque (logo/special use only)
  logo: {
    bold: 'BricolageGrotesque-Bold',
  },
  
  // Legacy fonts (keep for backward compatibility)
  legacy: {
    spaceMono: 'SpaceMono',
    systemMono: Platform.OS === 'ios' ? 'Menlo' : 'monospace',
  }
};

const SYSTEM_SANS_STACK =
  '-apple-system, BlinkMacSystemFont, "SF Pro Text", "Helvetica Neue", Helvetica, Arial, system-ui, sans-serif';

const SYSTEM_MONO_STACK =
  'ui-monospace, "SF Mono", SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace';

const isDesktopWeb = () => {
  if (Platform.OS !== 'web') return false;
  if (typeof window === 'undefined') return false;

  if (typeof window.matchMedia === 'function') {
    if (window.matchMedia('(hover: hover) and (pointer: fine), (min-width: 900px)').matches) return true;
  }

  return window.innerWidth >= 900;
};

// Helper functions for easy access to font families
export const getDefaultFont = (weight: 'regular' | 'italic' | 'semiBold' = 'regular') => {
  return FontFamilies.default[weight];
};

export const getMonoFont = (weight: 'regular' | 'italic' | 'semiBold' = 'regular') => {
  return FontFamilies.mono[weight];
};

export const getLogoFont = () => {
  return FontFamilies.logo.bold;
};

// Font weight mappings for the font families
export const FontWeights = {
  regular: '400',
  semiBold: '600', 
  bold: '700',
} as const;

// Style utilities for easy inline usage
export const Typography = {
  // Default font styles (IBM Plex Sans)
  default: (weight: 'regular' | 'italic' | 'semiBold' = 'regular') => ({
    ...(isDesktopWeb()
      ? {
          fontFamily: SYSTEM_SANS_STACK,
          ...(weight === 'italic' ? { fontStyle: 'italic' as const } : {}),
          ...(weight === 'semiBold' ? { fontWeight: '600' as const } : { fontWeight: '400' as const }),
        }
      : {
          fontFamily: getDefaultFont(weight),
        }),
  }),
  
  // Monospace font styles (IBM Plex Mono)
  mono: (weight: 'regular' | 'italic' | 'semiBold' = 'regular') => ({
    ...(isDesktopWeb()
      ? {
          fontFamily: SYSTEM_MONO_STACK,
          ...(weight === 'italic' ? { fontStyle: 'italic' as const } : {}),
          ...(weight === 'semiBold' ? { fontWeight: '600' as const } : { fontWeight: '400' as const }),
        }
      : {
          fontFamily: getMonoFont(weight),
        }),
  }),
  
  // Logo font style (Bricolage Grotesque)
  logo: () => ({
    fontFamily: getLogoFont(),
  }),
  
  // Header text style
  header: () => ({
    ...(isDesktopWeb()
      ? {
          fontFamily: SYSTEM_SANS_STACK,
          fontWeight: '600',
        }
      : {
          fontFamily: getDefaultFont('semiBold'),
        }),
  }),
  
  // Body text style
  body: () => ({
    ...(isDesktopWeb()
      ? {
          fontFamily: SYSTEM_SANS_STACK,
          fontWeight: '400',
        }
      : {
          fontFamily: getDefaultFont('regular'),
        }),
  }),
  
  // Legacy font styles (for backward compatibility)
  legacy: {
    spaceMono: () => ({
      fontFamily: FontFamilies.legacy.spaceMono,
    }),
    systemMono: () => ({
      fontFamily: FontFamilies.legacy.systemMono,
    }),
  }
};
