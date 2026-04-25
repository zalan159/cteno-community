import * as React from 'react';
import { useSession, useSessionMessages } from "@/sync/storage";
import { ActivityIndicator, FlatList, Platform, Pressable, View } from 'react-native';
import { useCallback } from 'react';
import { useHeaderHeight } from '@/utils/responsive';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { MessageView } from './MessageView';
import { Metadata, Session } from '@/sync/storageTypes';
import { ChatFooter } from './ChatFooter';
import { Message } from '@/sync/typesMessage';
import { sync } from '@/sync/sync';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Text } from '@/components/StyledText';
import { MarkdownView } from './markdown/MarkdownView';
import { Typography } from '@/constants/Typography';
import { layout } from './layout';

export const ChatList = React.memo((props: { session: Session }) => {
    const { messages, hasOlderMessages, isLoadingOlder } = useSessionMessages(props.session.id);
    return (
        <ChatListInternal
            metadata={props.session.metadata}
            sessionId={props.session.id}
            messages={messages}
            hasOlderMessages={hasOlderMessages}
            isLoadingOlder={isLoadingOlder}
        />
    );
});

const ListFooter = React.memo((props: { sessionId: string }) => {
    const session = useSession(props.sessionId)!;
    return (
        <>
            <StreamingBubble sessionId={props.sessionId} />
            <ChatFooter controlledByUser={session.agentState?.controlledByUser || false} />
        </>
    );
});

/** Renders streaming text/thinking deltas in real-time as they arrive via SSE */
const StreamingBubble = React.memo((props: { sessionId: string }) => {
    const session = useSession(props.sessionId);
    const { theme } = useUnistyles();
    const streamingText = session?.streamingText;
    const streamingThinking = session?.streamingThinking;
    const streamingNotice = session?.streamingNotice;

    if (!streamingText && !streamingThinking && !streamingNotice) return null;

    return (
        <View style={streamingStyles.container}>
            <View style={streamingStyles.content}>
                {streamingThinking ? (
                    <View style={[streamingStyles.thinkingContainer, { backgroundColor: theme.colors.surfaceHigh, borderColor: theme.colors.divider }]}>
                        <View style={[streamingStyles.thinkingHeader, { backgroundColor: theme.colors.surfaceHighest, borderBottomColor: theme.colors.divider }]}>
                            <Text style={[streamingStyles.thinkingLabel, { color: theme.colors.textSecondary }]}>Thinking...</Text>
                        </View>
                        <View style={streamingStyles.thinkingBody}>
                            <MarkdownView markdown={streamingThinking} />
                        </View>
                    </View>
                ) : null}
                {streamingText ? (
                    <View style={streamingStyles.textContainer}>
                        <MarkdownView markdown={streamingText} />
                    </View>
                ) : null}
                {streamingNotice ? (
                    <View style={streamingStyles.textContainer}>
                        <MarkdownView markdown={streamingNotice} />
                    </View>
                ) : null}
            </View>
        </View>
    );
});

const streamingStyles = StyleSheet.create((theme) => ({
    container: {
        flexDirection: 'row',
        justifyContent: 'center',
    },
    content: {
        flexDirection: 'column',
        flexGrow: 1,
        flexBasis: 0,
        maxWidth: layout.maxWidth,
    },
    textContainer: {
        marginHorizontal: 16,
        marginBottom: 12,
        borderRadius: 16,
        alignSelf: 'flex-start',
    },
    thinkingContainer: {
        marginHorizontal: 16,
        marginBottom: 12,
        borderRadius: 12,
        borderWidth: 1,
        overflow: 'hidden',
        alignSelf: 'flex-start',
        maxWidth: '100%',
    },
    thinkingHeader: {
        paddingHorizontal: 12,
        paddingVertical: 6,
        borderBottomWidth: 1,
    },
    thinkingLabel: {
        fontSize: 12,
        ...Typography.mono(),
    },
    thinkingBody: {
        paddingHorizontal: 12,
        paddingVertical: 6,
    },
}));

const LoadingOlderIndicator = React.memo(() => {
    const { theme } = useUnistyles();
    return (
        <View style={{ paddingVertical: 16, alignItems: 'center' }}>
            <ActivityIndicator size="small" color={theme.colors.textSecondary} />
        </View>
    );
});

const LoadOlderButton = React.memo((props: {
    hasServerMessages: boolean;
    isLoading: boolean;
    onPress: () => void;
}) => {
    const { theme } = useUnistyles();

    if (props.isLoading) {
        return <LoadingOlderIndicator />;
    }

    if (!props.hasServerMessages) {
        return null;
    }

    return (
        <Pressable
            onPress={props.onPress}
            style={{
                paddingVertical: 12,
                paddingHorizontal: 16,
                alignItems: 'center',
            }}
        >
            <Text style={{
                color: theme.colors.textSecondary,
                fontSize: 13,
            }}>
                加载更早
            </Text>
        </Pressable>
    );
});

const ChatListInternal = React.memo((props: {
    metadata: Metadata | null,
    sessionId: string,
    messages: Message[],
    hasOlderMessages: boolean,
    isLoadingOlder: boolean,
}) => {
    const headerHeight = useHeaderHeight();
    const safeArea = useSafeAreaInsets();

    const keyExtractor = useCallback((item: Message) => item.id, []);
    const renderItem = useCallback(({ item }: { item: Message }) => (
        <MessageView message={item} metadata={props.metadata} sessionId={props.sessionId} />
    ), [props.metadata, props.sessionId]);

    const handleLoadOlder = useCallback(() => {
        if (!props.hasOlderMessages || props.isLoadingOlder) {
            return;
        }
        sync.loadOlderMessages(props.sessionId);
    }, [props.hasOlderMessages, props.isLoadingOlder, props.sessionId]);

    return (
        <FlatList
            data={props.messages}
            keyExtractor={keyExtractor}
            maintainVisibleContentPosition={{
                minIndexForVisible: 0,
                autoscrollToTopThreshold: 10,
            }}
            keyboardShouldPersistTaps="handled"
            keyboardDismissMode={Platform.OS === 'ios' ? 'interactive' : 'none'}
            renderItem={renderItem}
            contentContainerStyle={{
                paddingTop: headerHeight + safeArea.top + 32,
            }}
            ListHeaderComponent={
                <LoadOlderButton
                    hasServerMessages={props.hasOlderMessages}
                    isLoading={props.isLoadingOlder}
                    onPress={handleLoadOlder}
                />
            }
            ListFooterComponent={<ListFooter sessionId={props.sessionId} />}
        />
    );
});
