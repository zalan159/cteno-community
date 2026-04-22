export function extractMessageText(raw: unknown): string {
  if (!raw || typeof raw !== 'object') {
    return '';
  }

  const candidate = raw as {
    content?: unknown;
    message?: { content?: unknown };
    text?: unknown;
  };

  if (typeof candidate.text === 'string') {
    return candidate.text;
  }

  const content = candidate.message?.content ?? candidate.content;

  if (typeof content === 'string') {
    return content;
  }

  if (!Array.isArray(content)) {
    return '';
  }

  const chunks = content
    .map(block => {
      if (!block || typeof block !== 'object') {
        return '';
      }

      const typedBlock = block as { type?: unknown; text?: unknown; content?: unknown };
      if (typedBlock.type === 'text' && typeof typedBlock.text === 'string') {
        return typedBlock.text;
      }

      if (Array.isArray(typedBlock.content)) {
        return typedBlock.content
          .map(inner => {
            if (!inner || typeof inner !== 'object') {
              return '';
            }
            const innerBlock = inner as { type?: unknown; text?: unknown };
            return innerBlock.type === 'text' && typeof innerBlock.text === 'string'
              ? innerBlock.text
              : '';
          })
          .filter(Boolean)
          .join('\n');
      }

      return '';
    })
    .filter(Boolean);

  return chunks.join('\n').trim();
}

export function normalizeAgentNames(agents: unknown): string[] {
  if (!Array.isArray(agents)) {
    return [];
  }

  return agents
    .map(agent => {
      if (typeof agent === 'string') {
        return agent;
      }
      if (agent && typeof agent === 'object' && typeof (agent as { name?: unknown }).name === 'string') {
        return (agent as { name: string }).name;
      }
      return '';
    })
    .filter(Boolean);
}
