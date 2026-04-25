import * as React from "react";
import { StyleSheet, View } from "react-native";
import { Text, uiColors } from "./primitives";

export interface MarkdownSpan {
  text: string;
  styles: Array<"bold" | "italic" | "code">;
  url: string | null;
}

export interface MarkdownBlock {
  kind: "paragraph" | "heading" | "code" | "quote" | "list";
  text: string;
  language?: string | null;
  spans?: MarkdownSpan[];
}

const spanPattern = /(\*\*(.*?)(?:\*\*|$))|(\*(.*?)(?:\*|$))|(\[([^\]]+)\](?:\(([^)]+)\))?)|(`(.*?)(?:`|$))/g;

export function parseMarkdownSpans(markdown: string, header = false): MarkdownSpan[] {
  const spans: MarkdownSpan[] = [];
  let lastIndex = 0;
  let match: RegExpExecArray | null;
  spanPattern.lastIndex = 0;
  while ((match = spanPattern.exec(markdown)) !== null) {
    const plainText = markdown.slice(lastIndex, match.index);
    if (plainText) spans.push({ styles: [], text: plainText, url: null });
    if (match[1]) spans.push({ styles: header ? [] : ["bold"], text: match[2], url: null });
    else if (match[3]) spans.push({ styles: header ? [] : ["italic"], text: match[4], url: null });
    else if (match[5]) spans.push({ styles: [], text: match[6], url: match[7] ?? null });
    else if (match[8]) spans.push({ styles: ["code"], text: match[9], url: null });
    lastIndex = spanPattern.lastIndex;
  }
  if (lastIndex < markdown.length) spans.push({ styles: [], text: markdown.slice(lastIndex), url: null });
  return spans;
}

export function parseMarkdown(markdown: string): MarkdownBlock[] {
  const lines = markdown.replace(/\r\n/g, "\n").split("\n");
  const blocks: MarkdownBlock[] = [];
  let code: string[] | null = null;
  let language: string | null = null;
  for (const line of lines) {
    if (line.startsWith("```")) {
      if (code) {
        blocks.push({ kind: "code", text: code.join("\n"), language });
        code = null;
        language = null;
      } else {
        code = [];
        language = line.slice(3).trim() || null;
      }
      continue;
    }
    if (code) {
      code.push(line);
      continue;
    }
    if (!line.trim()) continue;
    if (line.startsWith("#")) {
      const text = line.replace(/^#+\s*/, "");
      blocks.push({ kind: "heading", text, spans: parseMarkdownSpans(text, true) });
    } else if (line.startsWith(">")) {
      const text = line.replace(/^>\s*/, "");
      blocks.push({ kind: "quote", text, spans: parseMarkdownSpans(text) });
    } else if (/^\s*[-*]\s+/.test(line)) {
      const text = line.replace(/^\s*[-*]\s+/, "");
      blocks.push({ kind: "list", text, spans: parseMarkdownSpans(text) });
    } else {
      blocks.push({ kind: "paragraph", text: line, spans: parseMarkdownSpans(line) });
    }
  }
  if (code) blocks.push({ kind: "code", text: code.join("\n"), language });
  return blocks;
}

export function MarkdownView(props: { markdown: string }) {
  return (
    <View style={styles.wrap}>
      {parseMarkdown(props.markdown).map((block, index) => {
        if (block.kind === "code") {
          return (
            <View key={index} style={styles.code}>
              <Text style={styles.codeText}>{block.text}</Text>
            </View>
          );
        }
        return (
          <Text
            key={index}
            style={[
              block.kind === "heading" && styles.heading,
              block.kind === "quote" && styles.quote,
              block.kind === "list" && styles.list,
            ]}
          >
            {block.kind === "list" ? "• " : ""}
            {(block.spans ?? [{ text: block.text, styles: [], url: null }]).map((span, spanIndex) => (
              <Text
                key={spanIndex}
                style={[
                  span.styles.includes("bold") && styles.bold,
                  span.styles.includes("italic") && styles.italic,
                  span.styles.includes("code") && styles.inlineCode,
                  span.url && styles.link,
                ]}
              >
                {span.text}
              </Text>
            ))}
          </Text>
        );
      })}
    </View>
  );
}

const styles = StyleSheet.create({
  wrap: {
    gap: 6,
  },
  heading: {
    fontFamily: "IBMPlexSans-SemiBold",
    fontSize: 17,
  },
  quote: {
    borderLeftColor: uiColors.line,
    borderLeftWidth: 3,
    color: uiColors.muted,
    paddingLeft: 10,
  },
  list: {
    paddingLeft: 4,
  },
  code: {
    backgroundColor: "#14181D",
    borderRadius: 6,
    padding: 10,
  },
  codeText: {
    color: "#D9E2EC",
    fontFamily: "IBMPlexMono-Regular",
  },
  bold: {
    fontFamily: "IBMPlexSans-SemiBold",
  },
  italic: {
    fontStyle: "italic",
  },
  inlineCode: {
    backgroundColor: "#EDF2F6",
    borderRadius: 4,
    fontFamily: "IBMPlexMono-Regular",
  },
  link: {
    color: uiColors.accent,
  },
});
