import React, { useCallback, useState } from 'react';
import { View, Platform } from 'react-native';
import { useRouter } from 'expo-router';
import { useUnistyles } from 'react-native-unistyles';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { Typography } from '@/constants/Typography';
import { Text } from '@/components/StyledText';
import { ChatHeaderView } from '@/components/ChatHeaderView';
import { ChatList } from '@/components/ChatList';
import { AgentContentView } from '@/components/AgentContentView';
import { PersonaEmptyState } from '@/components/PersonaEmptyState';
import { PersonaChatInput } from '@/components/PersonaChatInput';
import { Deferred } from '@/components/Deferred';
import { useSession, useSessionMessages } from '@/sync/storage';
import { useHeaderHeight } from '@/utils/responsive';
import { isRunningOnMac } from '@/utils/platform';
import { useDemoSession } from '@/demo/useDemoSession';
import { setDemoMode } from '@/demo/demoMode';
import { t } from '@/text';

export default function DemoPage() {
    const { theme } = useUnistyles();
    const router = useRouter();
    const safeArea = useSafeAreaInsets();
    const headerHeight = useHeaderHeight();
    const [message, setMessage] = useState('');

    const { sendMessage, sessionId } = useDemoSession();
    const session = useSession(sessionId);
    const { messages } = useSessionMessages(sessionId);

    const handleSend = useCallback(() => {
        if (message.trim()) {
            const text = message;
            setMessage('');
            sendMessage(text);
        }
    }, [message, sendMessage]);

    const handleBack = useCallback(() => {
        setDemoMode(false);
        router.back();
    }, [router]);

    const header = (
        <View style={{
            position: 'absolute',
            top: 0,
            left: 0,
            right: 0,
            zIndex: 1000,
        }}>
            <ChatHeaderView
                title="Cteno"
                subtitle={t('demo.subtitle')}
                onBackPress={handleBack}
                isConnected={true}
            />
            {/* Demo mode banner */}
            <View style={{
                backgroundColor: theme.colors.button.primary.background,
                paddingVertical: 6,
                paddingHorizontal: 16,
                alignItems: 'center',
            }}>
                <Text style={{
                    color: '#fff',
                    fontSize: 13,
                    ...Typography.default('semiBold'),
                }}>
                    {t('demo.banner')}
                </Text>
            </View>
        </View>
    );

    const bannerHeight = 30; // approximate banner height

    const content = (
        <Deferred>
            {session && messages.length > 0 && (
                <ChatList session={session} />
            )}
        </Deferred>
    );

    const placeholder = (!session || messages.length === 0) ? (
        <PersonaEmptyState
            name="Cteno"
            description={t('demo.emptyDescription')}
            avatarId="default"
        />
    ) : null;

    const input = (
        <PersonaChatInput
            placeholder={t('demo.inputPlaceholder')}
            value={message}
            onChangeText={setMessage}
            onSend={handleSend}
            showAbortButton={false}
            connectionStatus={{
                text: t('demo.modeLabel'),
                color: theme.colors.status.connected,
                dotColor: theme.colors.status.connected,
                isPulsing: false,
            }}
        />
    );

    return (
        <>
            {header}
            <View style={{
                flex: 1,
                paddingTop: safeArea.top + headerHeight + bannerHeight,
                paddingBottom: safeArea.bottom + ((isRunningOnMac() || Platform.OS === 'web') ? 32 : 0),
            }}>
                <AgentContentView
                    content={content}
                    input={input}
                    placeholder={placeholder}
                />
            </View>
        </>
    );
}
