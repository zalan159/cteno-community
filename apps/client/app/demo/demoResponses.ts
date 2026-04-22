/**
 * Demo response generator for App Store review demo mode.
 *
 * Provides canned AI responses that demonstrate the app's capabilities
 * without needing a real backend connection.
 */

interface DemoResponse {
    text: string;
    /** Optional simulated tool call to show before the text response */
    toolCall?: {
        name: string;
        description: string;
        input: any;
        result: string;
    };
}

const responses: DemoResponse[] = [
    {
        text: "Hello! I'm Cteno, your personal AI assistant. In the full version, I can:\n\n- **Run shell commands** on your computer\n- **Read and edit files** across your projects\n- **Search the web** for up-to-date information\n- **Manage scheduled tasks** that run automatically\n- **Use custom skills** from the skill store\n\nThis is a demo to show you how the interface works. Sign in to unlock all features!",
    },
    {
        toolCall: {
            name: 'shell',
            description: 'List files in the current directory',
            input: { command: 'ls -la' },
            result: 'total 48\ndrwxr-xr-x  12 user  staff   384 Mar  9 10:00 .\ndrwxr-xr-x   5 user  staff   160 Mar  9 09:00 ..\n-rw-r--r--   1 user  staff  1024 Mar  9 10:00 README.md\n-rw-r--r--   1 user  staff  2048 Mar  9 10:00 package.json\ndrwxr-xr-x   8 user  staff   256 Mar  9 10:00 src\ndrwxr-xr-x   4 user  staff   128 Mar  9 10:00 docs',
        },
        text: "Here's what I found in your project directory. I can navigate your file system, run build commands, execute scripts, and much more. In the full version, I work directly on your computer with your real files.",
    },
    {
        toolCall: {
            name: 'websearch',
            description: 'Search the web',
            input: { query: 'latest AI news 2026' },
            result: 'Found 5 relevant results about the latest developments in AI technology...',
        },
        text: "I can search the web for real-time information and summarize results for you. This is useful for research, staying up to date, or finding documentation.\n\nIn the full version, web search results are integrated directly into our conversation context.",
    },
    {
        toolCall: {
            name: 'edit',
            description: 'Edit a source file',
            input: { file: 'src/app.tsx', old_string: 'Hello World', new_string: 'Hello Cteno' },
            result: 'Successfully edited src/app.tsx (1 replacement made)',
        },
        text: "I can make precise edits to your code files. I understand code context and can refactor, fix bugs, add features, and more.\n\nSign in to start using these capabilities on your real projects!",
    },
    {
        text: "I can also:\n\n1. **Schedule recurring tasks** - e.g., \"Check my server status every morning\"\n2. **Manage multiple personas** - Create different AI personalities for different workflows\n3. **Remember context** - I build up memory across conversations\n4. **Work with images** - Analyze screenshots, diagrams, and photos\n\nAll of this runs locally on your machine with end-to-end encryption. Your data stays yours.",
    },
];

let responseIndex = 0;

/**
 * Get the next demo response for a given user message.
 * Cycles through canned responses.
 */
export function getDemoResponse(_userMessage: string): DemoResponse {
    const response = responses[responseIndex % responses.length];
    responseIndex++;
    return response;
}

/**
 * Reset the response cycle (e.g., when re-entering demo mode).
 */
export function resetDemoResponses() {
    responseIndex = 0;
}
