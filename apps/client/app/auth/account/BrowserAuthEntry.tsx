import * as React from 'react';
import { Alert, Platform, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';

import { useAuth } from '@/auth/AuthContext';
import { AnimatedGeometricBackground } from '@/components/AnimatedGeometricBackground';
import { HomeHeaderNotAuth } from '@/components/HomeHeader';
import { RoundButton } from '@/components/RoundButton';
import { Text } from '@/components/StyledText';
import { Typography } from '@/constants/Typography';
import { loginWithBrowserOAuth } from '@/auth/account/authBrowser';
import { MobileLoginPage } from '@/auth/account/MobileLoginPage';

type BrowserLoginMode = 'cloud' | 'local-token';

const stylesheet = StyleSheet.create((theme) => ({
    container: {
        flex: 1,
        backgroundColor: theme.colors.groupped.background,
    },
    content: {
        flex: 1,
        paddingHorizontal: 24,
        justifyContent: 'center',
        alignItems: 'center',
    },
    panel: {
        width: '100%',
        maxWidth: 420,
        borderRadius: 24,
        padding: 28,
        backgroundColor: theme.colors.surface,
        borderWidth: 1,
        borderColor: theme.colors.divider,
        gap: 16,
    },
    title: {
        fontSize: 28,
        color: theme.colors.text,
        textAlign: 'center',
        ...Typography.default('semiBold'),
    },
    subtitle: {
        fontSize: 15,
        lineHeight: 22,
        color: theme.colors.textSecondary,
        textAlign: 'center',
        ...Typography.default(),
    },
    error: {
        fontSize: 14,
        lineHeight: 20,
        color: theme.colors.textDestructive,
        textAlign: 'center',
        ...Typography.default(),
    },
}));

interface BrowserAuthEntryProps {
    title?: string;
    subtitle?: string;
    buttonTitle?: string;
    loginMode?: BrowserLoginMode;
}

export function BrowserAuthEntry({
    title = 'Login',
    subtitle = 'Continue in your browser to sign in and return to the desktop app.',
    buttonTitle = 'Login',
    loginMode = 'cloud',
}: BrowserAuthEntryProps) {
    if (Platform.OS === 'ios' || Platform.OS === 'android') {
        return (
            <MobileLoginPage
                title={title}
                subtitle={subtitle}
                buttonTitle={buttonTitle}
                loginMode={loginMode}
            />
        );
    }

    const styles = stylesheet;
    const auth = useAuth();
    const [loading, setLoading] = React.useState(false);
    const [error, setError] = React.useState<string | null>(null);

    const handleLogin = React.useCallback(async () => {
        setLoading(true);
        setError(null);

        try {
            const payload = await loginWithBrowserOAuth();
            if (loginMode === 'local-token') {
                await auth.loginForLocalToken(payload);
            } else {
                await auth.login(payload);
            }
        } catch (caughtError) {
            const message = caughtError instanceof Error ? caughtError.message : String(caughtError);
            console.error('Browser OAuth login failed:', caughtError);
            setError(message);

            if (Platform.OS !== 'web') {
                Alert.alert('Login failed', message);
            }
        } finally {
            setLoading(false);
        }
    }, [auth, loginMode]);

    return (
        <View style={styles.container}>
            <AnimatedGeometricBackground />
            <HomeHeaderNotAuth />
            <View style={styles.content}>
                <View style={styles.panel}>
                    <Text style={styles.title}>{title}</Text>
                    <Text style={styles.subtitle}>{subtitle}</Text>
                    <RoundButton
                        title={buttonTitle}
                        size="large"
                        onPress={handleLogin}
                        loading={loading}
                    />
                    {error ? <Text style={styles.error}>{error}</Text> : null}
                </View>
            </View>
        </View>
    );
}
