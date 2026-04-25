const { getDefaultConfig } = require("expo/metro-config");
const path = require("path");

// Explicitly set project root to avoid monorepo detection issues
const projectRoot = __dirname;

const config = getDefaultConfig(projectRoot, {
  // Enable CSS support for web
  isCSSEnabled: true,
});

// Override projectRoot and watchFolders to prevent monorepo path issues
config.projectRoot = projectRoot;
config.watchFolders = [
  projectRoot,
  path.resolve(__dirname, "../../packages/client-ui"),
  path.resolve(__dirname, "../../packages/client-sync"),
  path.resolve(__dirname, "../../packages/client-agent-ui"),
  path.resolve(__dirname, "../../packages/client-a2ui"),
];
config.resolver.extraNodeModules = {
  ...(config.resolver.extraNodeModules || {}),
  "@cteno/client-ui": path.resolve(__dirname, "../../packages/client-ui/src"),
  "@cteno/client-sync": path.resolve(__dirname, "../../packages/client-sync/src"),
  "@cteno/client-agent-ui": path.resolve(__dirname, "../../packages/client-agent-ui/src"),
  "@cteno/client-a2ui": path.resolve(__dirname, "../../packages/client-a2ui/src"),
};

// Add support for .wasm files (required by Skia for all platforms)
// Source: https://shopify.github.io/react-native-skia/docs/getting-started/installation/
config.resolver.assetExts.push('wasm');

// Enable inlineRequires for proper Skia and Reanimated loading
// Source: https://shopify.github.io/react-native-skia/docs/getting-started/web/
// Without this, Skia throws "react-native-reanimated is not installed" error
// This is cross-platform compatible (iOS, Android, web)
config.transformer.getTransformOptions = async () => ({
  transform: {
    experimentalImportSupport: false,
    inlineRequires: true, // Critical for @shopify/react-native-skia
  },
});

module.exports = config;
