import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";

const repoRoot = new URL("..", import.meta.url).pathname.replace(/\/$/, "");
const sharedRoots = [
  "packages/client-ui",
  "packages/client-sync",
  "packages/client-agent-ui",
  "packages/client-a2ui",
];
const liveAppRoot = "apps/live/app";

const violations = [];

for (const root of sharedRoots) {
  scan(join(repoRoot, root), (file, source) => {
    if (/from\s+["']expo-router["']|import\s+.*["']expo-router/.test(source)) {
      violations.push(`${relative(repoRoot, file)} imports expo-router`);
    }
    if (/apps\/client\/app|from\s+["']@\/|from\s+["']\.\.\/\.\.\/apps\/client/.test(source)) {
      violations.push(`${relative(repoRoot, file)} imports app-layer code`);
    }
    if (/packages\/commercial\/live-|@cteno\/live-/.test(source)) {
      violations.push(`${relative(repoRoot, file)} imports commercial live code`);
    }
  });
}

scan(join(repoRoot, liveAppRoot), (file, source) => {
  if (/apps\/client\/app|from\s+["']@\/(?!.*src)|from\s+["']\.\.\/\.\.\/client\/app/.test(source)) {
    violations.push(`${relative(repoRoot, file)} imports the generic client app`);
  }
});

if (violations.length > 0) {
  console.error("Client package boundary violations:");
  for (const violation of violations) console.error(`- ${violation}`);
  process.exit(1);
}

console.log("Client package boundaries OK");

function scan(dir, visit) {
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry);
    const stat = statSync(path);
    if (stat.isDirectory()) {
      if (entry === "node_modules" || entry === "dist") continue;
      scan(path, visit);
    } else if (/\.(ts|tsx|js|jsx|mjs|cjs)$/.test(entry)) {
      visit(path, readFileSync(path, "utf8"));
    }
  }
}
