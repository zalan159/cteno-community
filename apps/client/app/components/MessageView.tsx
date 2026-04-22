import * as React from "react";
import { Pressable, View, Image as RNImage, ActivityIndicator, Platform } from 'react-native';
import { RemoteImage } from '@/components/RemoteImage';
import { ImagePreviewModal } from '@/components/ImagePreviewModal';
import { StyleSheet } from 'react-native-unistyles';
import { MarkdownView } from "./markdown/MarkdownView";
import { t } from '@/text';
import { Message, UserTextMessage, AgentTextMessage, ToolCallMessage, ImageAttachment } from "@/sync/typesMessage";
import { Metadata } from "@/sync/storageTypes";
import { layout } from "./layout";
import { ToolView } from "./tools/ToolView";
import { AgentEvent } from "@/sync/typesRaw";
import { sync } from '@/sync/sync';
import { Option } from './markdown/MarkdownView';
import { Text } from '@/components/StyledText';
import { Typography } from '@/constants/Typography';
import { getImageDownloadUrl } from '@/sync/apiFiles';
import { useAuth } from '@/auth/AuthContext';
import { useRouter } from 'expo-router';
import { useSession } from '@/sync/storage';

export const MessageView = (props: {
  message: Message;
  metadata: Metadata | null;
  sessionId: string;
  getMessageById?: (id: string) => Message | null;
}) => {
  return (
    <View style={styles.messageContainer} renderToHardwareTextureAndroid={true}>
      <View style={styles.messageContent}>
        <RenderBlock
          message={props.message}
          metadata={props.metadata}
          sessionId={props.sessionId}
          getMessageById={props.getMessageById}
        />
      </View>
    </View>
  );
};

// RenderBlock function that dispatches to the correct component based on message kind
function RenderBlock(props: {
  message: Message;
  metadata: Metadata | null;
  sessionId: string;
  getMessageById?: (id: string) => Message | null;
}): React.ReactElement {
  switch (props.message.kind) {
    case 'user-text':
      return <UserTextBlock message={props.message} sessionId={props.sessionId} />;

    case 'agent-text':
      return <AgentTextBlock message={props.message} sessionId={props.sessionId} />;

    case 'tool-call':
      return <ToolCallBlock
        message={props.message}
        metadata={props.metadata}
        sessionId={props.sessionId}
        getMessageById={props.getMessageById}
      />;

    case 'agent-event':
      return <AgentEventBlock event={props.message.event} metadata={props.metadata} />;


    default:
      // Exhaustive check - TypeScript will error if we miss a case
      const _exhaustive: never = props.message;
      throw new Error(`Unknown message kind: ${_exhaustive}`);
  }
}

function ChatImage(props: { image: ImageAttachment; onPress: (uri: string) => void }) {
  const { image, onPress } = props;

  // Inline base64
  if (image.data) {
    const uri = `data:${image.media_type};base64,${image.data}`;
    return (
      <Pressable onPress={() => onPress(uri)}>
        <RNImage source={{ uri }} style={styles.userImage} resizeMode="cover" />
      </Pressable>
    );
  }

  // OSS file reference
  if (image.file_id) {
    return <FileRefImage fileId={image.file_id} onPress={onPress} />;
  }

  return null;
}

function FileRefImage(props: { fileId: string; onPress: (uri: string) => void }) {
  const [thumbUri, setThumbUri] = React.useState<string | null>(null);
  const [error, setError] = React.useState(false);
  const { credentials } = useAuth();

  // Load thumbnail (240px = 120pt * 2x retina)
  React.useEffect(() => {
    let cancelled = false;
    if (!credentials) {
      setError(true);
      return;
    }
    getImageDownloadUrl(credentials, props.fileId, 240)
      .then(url => { if (!cancelled) setThumbUri(url); })
      .catch(() => { if (!cancelled) setError(true); });
    return () => { cancelled = true; };
  }, [props.fileId, credentials]);

  const handlePress = React.useCallback(async () => {
    if (!credentials) return;
    try {
      // Load full-size URL on press
      const fullUrl = await getImageDownloadUrl(credentials, props.fileId);
      props.onPress(fullUrl);
    } catch {
      // Fallback to thumbnail
      if (thumbUri) props.onPress(thumbUri);
    }
  }, [credentials, props.fileId, props.onPress, thumbUri]);

  if (error) {
    return (
      <View style={[styles.userImage, { justifyContent: 'center', alignItems: 'center', backgroundColor: '#333' }]}>
        <Text style={{ color: '#999', fontSize: 12 }}>Failed to load</Text>
      </View>
    );
  }

  if (!thumbUri) {
    return (
      <View style={[styles.userImage, { justifyContent: 'center', alignItems: 'center', backgroundColor: '#222' }]}>
        <ActivityIndicator size="small" color="#666" />
      </View>
    );
  }

  return (
    <Pressable onPress={handlePress}>
      <RemoteImage uri={thumbUri} style={styles.userImage} resizeMode="cover" />
    </Pressable>
  );
}

function UserTextBlock(props: {
  message: UserTextMessage;
  sessionId: string;
}) {
  const handleOptionPress = React.useCallback((option: Option) => {
    sync.sendMessage(props.sessionId, option.title);
  }, [props.sessionId]);

  const [previewUri, setPreviewUri] = React.useState<string | null>(null);

  return (
    <View style={styles.userMessageContainer}>
      <View style={styles.userMessageBubble}>
        {props.message.images && props.message.images.length > 0 && (
          <View style={styles.userImageRow}>
            {props.message.images.map((img, idx) => (
              <ChatImage key={idx} image={img} onPress={setPreviewUri} />
            ))}
          </View>
        )}
        <MarkdownView markdown={props.message.displayText || props.message.text} onOptionPress={handleOptionPress} />
      </View>
      {previewUri && (
        <ImagePreviewModal uri={previewUri} visible onClose={() => setPreviewUri(null)} />
      )}
    </View>
  );
}

function AgentTextBlock(props: {
  message: AgentTextMessage;
  sessionId: string;
}) {
  const router = useRouter();
  const session = useSession(props.sessionId);
  const handleOptionPress = React.useCallback((option: Option) => {
    sync.sendMessage(props.sessionId, option.title);
  }, [props.sessionId]);
  const handleLocalFilePress = React.useCallback((rawPath: string) => {
    const homeDir = session?.metadata?.homeDir;
    const resolvedPath = rawPath.startsWith('~/') && homeDir
      ? `${homeDir}/${rawPath.slice(2)}`
      : rawPath;
    const encodedPath = btoa(resolvedPath);
    router.push(`/session/${props.sessionId}/file?path=${encodedPath}`);
  }, [props.sessionId, router, session?.metadata?.homeDir]);

  const [open, setOpen] = React.useState(true);
  React.useEffect(() => {
    // Reset disclosure state when list virtualization swaps this component to a different message.
    setOpen(true);
  }, [props.message.id]);

  // Thinking messages are always shown, but collapsed by default.
  if (props.message.isThinking) {
    return (
      <View style={styles.thinkingContainer}>
        <Pressable
          onPress={() => setOpen((v) => !v)}
          style={styles.thinkingHeader}
          hitSlop={8}
          accessibilityRole="button"
          accessibilityLabel="thinking"
          accessibilityState={{ expanded: open }}
        >
          <View style={styles.thinkingHeaderRow}>
            <Text style={styles.thinkingLabel}>{`${t('sessionInfo.thinking')}...`}</Text>
            <Text style={styles.thinkingChevron}>{open ? 'v' : '>'}</Text>
          </View>
        </Pressable>
        {open ? (
          <View style={styles.thinkingBody}>
            <MarkdownView
              markdown={props.message.text ?? ''}
              onOptionPress={handleOptionPress}
              onLocalFilePress={handleLocalFilePress}
            />
          </View>
        ) : null}
      </View>
    );
  }

  const [previewUri, setPreviewUri] = React.useState<string | null>(null);

  return (
    <View style={styles.agentMessageContainer}>
      {props.message.text ? (
        <MarkdownView
          markdown={props.message.text}
          onOptionPress={handleOptionPress}
          onLocalFilePress={handleLocalFilePress}
        />
      ) : null}
      {props.message.images && props.message.images.length > 0 && (
        <View style={styles.agentImageRow}>
          {props.message.images.map((img, idx) => (
            <ChatImage key={idx} image={img} onPress={setPreviewUri} />
          ))}
        </View>
      )}
      {previewUri && (
        <ImagePreviewModal uri={previewUri} visible onClose={() => setPreviewUri(null)} />
      )}
    </View>
  );
}

function AgentEventBlock(props: {
  event: AgentEvent;
  metadata: Metadata | null;
}): React.ReactElement | null {
  if (props.event.type === 'switch') {
    return (
      <View style={styles.agentEventContainer}>
        <Text style={styles.agentEventText}>{t('message.switchedToMode', { mode: props.event.mode })}</Text>
      </View>
    );
  }
  if (props.event.type === 'message') {
    return (
      <View style={styles.agentEventContainer}>
        <Text style={styles.agentEventText}>{props.event.message}</Text>
      </View>
    );
  }
  if (props.event.type === 'limit-reached') {
    const formatTime = (timestamp: number): string => {
      try {
        const date = new Date(timestamp * 1000); // Convert from Unix timestamp
        return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
      } catch {
        return t('message.unknownTime');
      }
    };

    return (
      <View style={styles.agentEventContainer}>
        <Text style={styles.agentEventText}>
          {t('message.usageLimitUntil', { time: formatTime(props.event.endsAt) })}
        </Text>
      </View>
    );
  }
  return (
    <View style={styles.agentEventContainer}>
      <Text style={styles.agentEventText}>{t('message.unknownEvent')}</Text>
    </View>
  );
}

function ToolCallBlock(props: {
  message: ToolCallMessage;
  metadata: Metadata | null;
  sessionId: string;
  getMessageById?: (id: string) => Message | null;
}) {
  if (!props.message.tool) {
    return null;
  }
  return (
    <View style={styles.toolContainer}>
      <ToolView
        tool={props.message.tool}
        metadata={props.metadata}
        messages={props.message.children}
        sessionId={props.sessionId}
        messageId={props.message.id}
      />
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  messageContainer: {
    flexDirection: 'row',
    justifyContent: 'center',
  },
  messageContent: {
    flexDirection: 'column',
    flexGrow: 1,
    flexBasis: 0,
    maxWidth: layout.maxWidth,
  },
  userMessageContainer: {
    maxWidth: '100%',
    flexDirection: 'column',
    alignItems: 'flex-end',
    justifyContent: 'flex-end',
    paddingHorizontal: 16,
  },
  userMessageBubble: {
    backgroundColor: theme.colors.userMessageBackground,
    paddingHorizontal: 12,
    paddingVertical: 4,
    borderRadius: 12,
    marginBottom: 12,
    maxWidth: '100%',
  },
  userImageRow: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 8,
    marginTop: 8,
    marginBottom: 4,
  },
  agentImageRow: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 8,
    marginTop: 8,
    marginBottom: 4,
  },
  generatedImage: {
    width: 300,
    height: 300,
    borderRadius: 8,
  },
  userImage: {
    width: 120,
    height: 120,
    borderRadius: 8,
  },
  agentMessageContainer: {
    marginHorizontal: 16,
    marginBottom: 12,
    borderRadius: 16,
    alignSelf: 'flex-start',
  },
  thinkingContainer: {
    marginHorizontal: 16,
    marginBottom: 12,
    borderRadius: 12,
    backgroundColor: theme.colors.surfaceHigh,
    borderWidth: 1,
    borderColor: theme.colors.divider,
    overflow: 'hidden',
    alignSelf: 'flex-start',
    maxWidth: '100%',
  },
  thinkingHeader: {
    paddingHorizontal: 12,
    paddingVertical: 6,
    borderBottomWidth: 1,
    borderBottomColor: theme.colors.divider,
    backgroundColor: theme.colors.surfaceHighest,
  },
  thinkingHeaderRow: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: 8,
  },
  thinkingLabel: {
    fontSize: 12,
    color: theme.colors.textSecondary,
    ...Typography.mono(),
  },
  thinkingChevron: {
    fontSize: 12,
    color: theme.colors.textSecondary,
    ...Typography.mono(),
  },
  thinkingBody: {
    paddingHorizontal: 12,
    paddingVertical: 2,
    opacity: 0.78,
  },
  agentEventContainer: {
    marginHorizontal: 8,
    alignItems: 'center',
    paddingVertical: 8,
  },
  agentEventText: {
    color: theme.colors.agentEventText,
    fontSize: 14,
  },
  toolContainer: {
    marginHorizontal: 8,
  },
  debugText: {
    color: theme.colors.agentEventText,
    fontSize: 12,
  },
}));
