import * as AppleAuthentication from 'expo-apple-authentication';
import * as React from 'react';
import {
    Alert,
    KeyboardAvoidingView,
    Platform,
    Pressable,
    ScrollView,
    TextInput,
    View,
} from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';

import { useAuth } from '@/auth/AuthContext';
import {
    buildLandingForgotPasswordUrl,
    buildLandingPrivacyUrl,
    buildLandingTermsUrl,
    loginWithBrowserOAuth,
} from '@/auth/account/authBrowser';
import { signInWithApple } from '@/auth/account/appleSignIn';
import type { AuthSuccessPayload } from '@/auth/tokenStorage';
import { AnimatedGeometricBackground } from '@/components/AnimatedGeometricBackground';
import { HomeHeaderNotAuth } from '@/components/HomeHeader';
import { RoundButton } from '@/components/RoundButton';
import { Text } from '@/components/StyledText';
import { Typography } from '@/constants/Typography';
import { requireServerUrl } from '@/sync/serverConfig';
import { openExternalUrl } from '@/utils/openExternalUrl';

type LoginStep = 'email' | 'password' | 'social' | 'register';
type PendingAction = 'check-email' | 'login' | 'register' | 'google' | 'apple' | null;
type LoginMode = 'cloud' | 'local-token';

type EmailCheckResponse = {
    exists?: boolean;
    hasPassword?: boolean;
    error?: string;
};

type AuthResponse = {
    accessToken?: string;
    refreshToken?: string;
    expiresIn?: number | string;
    refreshExpiresIn?: number | string;
    userId?: string;
    token?: string;
    error?: string;
};

const stylesheet = StyleSheet.create((theme) => ({
    container: {
        flex: 1,
        backgroundColor: theme.colors.groupped.background,
    },
    scrollContent: {
        flexGrow: 1,
    },
    content: {
        flex: 1,
        paddingHorizontal: 20,
        paddingBottom: 32,
        justifyContent: 'center',
    },
    panel: {
        width: '100%',
        maxWidth: 460,
        alignSelf: 'center',
        borderRadius: 28,
        padding: 24,
        backgroundColor: theme.colors.surface,
        borderWidth: 1,
        borderColor: theme.colors.divider,
        gap: 18,
    },
    eyebrow: {
        fontSize: 12,
        letterSpacing: 1.2,
        textTransform: 'uppercase',
        color: theme.colors.textSecondary,
        ...Typography.default('semiBold'),
    },
    title: {
        fontSize: 30,
        color: theme.colors.text,
        ...Typography.default('semiBold'),
    },
    subtitle: {
        fontSize: 15,
        lineHeight: 22,
        color: theme.colors.textSecondary,
        ...Typography.default(),
    },
    fieldGroup: {
        gap: 10,
    },
    label: {
        fontSize: 13,
        color: theme.colors.textSecondary,
        ...Typography.default('semiBold'),
    },
    input: {
        height: 52,
        borderRadius: 18,
        borderWidth: 1,
        borderColor: theme.colors.divider,
        paddingHorizontal: 16,
        color: theme.colors.text,
        backgroundColor: theme.colors.groupped.background,
        ...Typography.default(),
    },
    inputDisabled: {
        color: theme.colors.textSecondary,
        opacity: 0.8,
    },
    helperText: {
        fontSize: 13,
        lineHeight: 18,
        color: theme.colors.textSecondary,
        ...Typography.default(),
    },
    infoBox: {
        borderRadius: 16,
        borderWidth: 1,
        borderColor: `${theme.colors.button.primary.background}33`,
        backgroundColor: `${theme.colors.button.primary.background}12`,
        paddingHorizontal: 14,
        paddingVertical: 12,
        gap: 6,
    },
    infoTitle: {
        fontSize: 14,
        color: theme.colors.text,
        ...Typography.default('semiBold'),
    },
    infoText: {
        fontSize: 14,
        lineHeight: 20,
        color: theme.colors.textSecondary,
        ...Typography.default(),
    },
    errorBox: {
        borderRadius: 16,
        borderWidth: 1,
        borderColor: `${theme.colors.textDestructive}33`,
        backgroundColor: `${theme.colors.textDestructive}14`,
        paddingHorizontal: 14,
        paddingVertical: 12,
    },
    errorText: {
        fontSize: 14,
        lineHeight: 20,
        color: theme.colors.textDestructive,
        ...Typography.default(),
    },
    noticeBox: {
        borderRadius: 16,
        borderWidth: 1,
        borderColor: `${theme.colors.button.primary.background}33`,
        backgroundColor: `${theme.colors.button.primary.background}14`,
        paddingHorizontal: 14,
        paddingVertical: 12,
    },
    noticeText: {
        fontSize: 14,
        lineHeight: 20,
        color: theme.colors.text,
        ...Typography.default(),
    },
    linkRow: {
        flexDirection: 'row',
        flexWrap: 'wrap',
        alignItems: 'center',
        justifyContent: 'space-between',
        gap: 12,
    },
    linkButton: {
        paddingVertical: 4,
    },
    linkText: {
        fontSize: 14,
        color: theme.colors.button.primary.background,
        ...Typography.default('semiBold'),
    },
    termsRow: {
        flexDirection: 'row',
        alignItems: 'flex-start',
        gap: 12,
    },
    checkbox: {
        width: 22,
        height: 22,
        marginTop: 1,
        borderRadius: 6,
        borderWidth: 1.5,
        borderColor: theme.colors.divider,
        backgroundColor: theme.colors.surfaceHighest,
        alignItems: 'center',
        justifyContent: 'center',
    },
    checkboxChecked: {
        borderColor: theme.colors.button.primary.background,
        backgroundColor: theme.colors.button.primary.background,
    },
    checkboxInner: {
        width: 8,
        height: 8,
        borderRadius: 999,
        backgroundColor: theme.colors.button.primary.tint,
    },
    termsContent: {
        flex: 1,
        gap: 4,
    },
    termsText: {
        fontSize: 13,
        lineHeight: 19,
        color: theme.colors.textSecondary,
        ...Typography.default(),
    },
    termsLinks: {
        flexDirection: 'row',
        flexWrap: 'wrap',
        gap: 14,
    },
    divider: {
        height: 1,
        backgroundColor: theme.colors.divider,
    },
    sectionTitle: {
        fontSize: 13,
        letterSpacing: 1,
        textTransform: 'uppercase',
        color: theme.colors.textSecondary,
        ...Typography.default('semiBold'),
    },
    providerList: {
        gap: 10,
    },
    providerButton: {
        minHeight: 50,
        borderRadius: 18,
        borderWidth: 1,
        borderColor: theme.colors.divider,
        paddingHorizontal: 16,
        backgroundColor: theme.colors.surfaceHighest,
        justifyContent: 'center',
    },
    providerButtonPressed: {
        opacity: 0.88,
    },
    providerButtonDisabled: {
        opacity: 0.5,
    },
    providerLabel: {
        fontSize: 15,
        color: theme.colors.text,
        ...Typography.default('semiBold'),
    },
    appleButtonWrap: {
        borderRadius: 18,
        overflow: 'hidden',
    },
    appleButtonDisabled: {
        opacity: 0.5,
    },
    appleButton: {
        width: '100%',
        height: 50,
    },
}));

function toPositiveNumber(value: unknown, fallback: number): number {
    if (typeof value === 'number' && Number.isFinite(value) && value > 0) {
        return value;
    }

    if (typeof value === 'string' && value.trim()) {
        const parsed = Number(value);
        if (Number.isFinite(parsed) && parsed > 0) {
            return parsed;
        }
    }

    return fallback;
}

function normalizeAuthPayload(payload: AuthResponse | null): AuthSuccessPayload | null {
    const accessToken = payload?.accessToken ?? payload?.token;
    if (typeof accessToken !== 'string' || !accessToken) {
        return null;
    }

    return {
        accessToken,
        refreshToken: typeof payload?.refreshToken === 'string' ? payload.refreshToken : '',
        expiresIn: toPositiveNumber(payload?.expiresIn, 60 * 60),
        refreshExpiresIn: toPositiveNumber(payload?.refreshExpiresIn, 60 * 24 * 3600),
        userId: typeof payload?.userId === 'string' ? payload.userId : '',
    };
}

interface MobileLoginPageProps {
    title?: string;
    subtitle?: string;
    buttonTitle?: string;
    loginMode?: LoginMode;
}

export function MobileLoginPage({
    title = 'Login',
    subtitle = 'Enter your email to continue, or use Apple or Google below.',
    buttonTitle = 'Login',
    loginMode = 'cloud',
}: MobileLoginPageProps) {
    const styles = stylesheet;
    const { theme } = useUnistyles();
    const auth = useAuth();
    const [step, setStep] = React.useState<LoginStep>('email');
    const [email, setEmail] = React.useState('');
    const [password, setPassword] = React.useState('');
    const [termsAccepted, setTermsAccepted] = React.useState(false);
    const [error, setError] = React.useState<string | null>(null);
    const [notice, setNotice] = React.useState<string | null>(null);
    const [pendingAction, setPendingAction] = React.useState<PendingAction>(null);

    const busy = pendingAction !== null;

    const completeLogin = React.useCallback(async (payload: AuthSuccessPayload) => {
        if (loginMode === 'local-token') {
            await auth.loginForLocalToken(payload);
        } else {
            await auth.login(payload);
        }
    }, [auth, loginMode]);

    const showNativeAlert = React.useCallback((titleText: string, message: string) => {
        if (Platform.OS === 'ios' || Platform.OS === 'android') {
            Alert.alert(titleText, message);
        }
    }, []);

    const resetFlow = React.useCallback(() => {
        setStep('email');
        setPassword('');
        setTermsAccepted(false);
        setError(null);
        setNotice(null);
    }, []);

    const handleEmailChange = React.useCallback((nextEmail: string) => {
        setEmail(nextEmail);
        setError(null);
        setNotice(null);
        if (step !== 'email') {
            setStep('email');
            setPassword('');
            setTermsAccepted(false);
        }
    }, [step]);

    const openLandingUrl = React.useCallback(async (url: string) => {
        try {
            await openExternalUrl(url);
        } catch (caughtError) {
            const message = caughtError instanceof Error ? caughtError.message : String(caughtError);
            setError(message);
            showNativeAlert('Unable to open link', message);
        }
    }, [showNativeAlert]);

    const handleCheckEmail = React.useCallback(async () => {
        setPendingAction('check-email');
        setError(null);
        setNotice(null);

        try {
            const response = await fetch(`${requireServerUrl()}/v1/auth/check-email`, {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/json',
                },
                body: JSON.stringify({
                    email: email.trim(),
                }),
            });

            const payload = await response.json().catch(() => null) as EmailCheckResponse | null;
            if (!response.ok || typeof payload?.exists !== 'boolean' || typeof payload?.hasPassword !== 'boolean') {
                throw new Error(payload?.error || `Email check failed with status ${response.status}.`);
            }

            setPassword('');
            setTermsAccepted(false);

            if (!payload.exists) {
                setStep('register');
                return;
            }

            if (payload.hasPassword) {
                setStep('password');
                return;
            }

            setStep('social');
        } catch (caughtError) {
            const message = caughtError instanceof Error ? caughtError.message : String(caughtError);
            console.error('Email check failed:', caughtError);
            setError(message);
            showNativeAlert('Unable to continue', message);
        } finally {
            setPendingAction(null);
        }
    }, [email, showNativeAlert]);

    const handlePasswordLogin = React.useCallback(async () => {
        setPendingAction('login');
        setError(null);
        setNotice(null);

        try {
            const response = await fetch(`${requireServerUrl()}/v1/auth/login`, {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/json',
                },
                body: JSON.stringify({
                    email: email.trim(),
                    password,
                }),
            });

            const rawPayload = await response.json().catch(() => null) as AuthResponse | null;
            const payload = normalizeAuthPayload(rawPayload);
            if (!response.ok || !payload) {
                throw new Error(rawPayload?.error || `Login failed with status ${response.status}.`);
            }

            await completeLogin(payload);
        } catch (caughtError) {
            const message = caughtError instanceof Error ? caughtError.message : String(caughtError);
            console.error('Email login failed:', caughtError);
            setError(message);
            showNativeAlert('Login failed', message);
        } finally {
            setPendingAction(null);
        }
    }, [completeLogin, email, password, showNativeAlert]);

    const handleRegister = React.useCallback(async () => {
        if (Platform.OS === 'ios' && !termsAccepted) {
            const message = 'Please accept the Terms of Service and Privacy Policy to continue.';
            setError(message);
            showNativeAlert('Terms required', message);
            return;
        }

        setPendingAction('register');
        setError(null);
        setNotice(null);

        try {
            const response = await fetch(`${requireServerUrl()}/v1/auth/register`, {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/json',
                },
                body: JSON.stringify({
                    email: email.trim(),
                    password,
                }),
            });

            const rawPayload = await response.json().catch(() => null) as AuthResponse | null;
            const payload = normalizeAuthPayload(rawPayload);
            if (!response.ok || !payload) {
                throw new Error(rawPayload?.error || `Registration failed with status ${response.status}.`);
            }

            const message = 'Verification email sent. Please check your inbox.';
            setNotice(message);
            showNativeAlert('Check your email', message);
            await completeLogin(payload);
        } catch (caughtError) {
            const message = caughtError instanceof Error ? caughtError.message : String(caughtError);
            console.error('Registration failed:', caughtError);
            setError(message);
            showNativeAlert('Registration failed', message);
        } finally {
            setPendingAction(null);
        }
    }, [completeLogin, email, password, showNativeAlert, termsAccepted]);

    const handleGoogleLogin = React.useCallback(async () => {
        setPendingAction('google');
        setError(null);
        setNotice(null);

        try {
            const payload = await loginWithBrowserOAuth();
            await completeLogin(payload);
        } catch (caughtError) {
            const message = caughtError instanceof Error ? caughtError.message : String(caughtError);
            console.error('Google OAuth login failed:', caughtError);
            setError(message);
            showNativeAlert('Login failed', message);
        } finally {
            setPendingAction(null);
        }
    }, [completeLogin, showNativeAlert]);

    const handleAppleLogin = React.useCallback(async () => {
        if (Platform.OS !== 'ios' || busy) {
            return;
        }

        setPendingAction('apple');
        setError(null);
        setNotice(null);

        try {
            const payload = await signInWithApple();
            if (!payload) {
                return;
            }

            await completeLogin(payload);
        } catch (caughtError) {
            const message = caughtError instanceof Error ? caughtError.message : String(caughtError);
            console.error('Apple login failed:', caughtError);
            setError(message);
            showNativeAlert('Login failed', message);
        } finally {
            setPendingAction(null);
        }
    }, [busy, completeLogin, showNativeAlert]);

    const emailLocked = step !== 'email';
    const socialPrompt = Platform.OS === 'ios'
        ? 'This email already uses social sign-in. Continue with Google or Apple below.'
        : 'This email already uses social sign-in. Continue with Google below.';

    return (
        <View style={styles.container}>
            <AnimatedGeometricBackground />
            <HomeHeaderNotAuth />
            <KeyboardAvoidingView
                style={styles.content}
                behavior={Platform.OS === 'ios' ? 'padding' : undefined}
            >
                <ScrollView
                    keyboardShouldPersistTaps="handled"
                    contentContainerStyle={styles.scrollContent}
                    showsVerticalScrollIndicator={false}
                >
                    <View style={styles.content}>
                        <View style={styles.panel}>
                            <Text style={styles.eyebrow}>Mobile Sign-In</Text>
                            <View>
                                <Text style={styles.title}>{title}</Text>
                                <Text style={styles.subtitle}>{subtitle}</Text>
                            </View>

                            <View style={styles.fieldGroup}>
                                <Text style={styles.label}>Email</Text>
                                <TextInput
                                    autoCapitalize="none"
                                    autoCorrect={false}
                                    autoComplete="email"
                                    editable={!emailLocked && !busy}
                                    keyboardType="email-address"
                                    placeholder="you@example.com"
                                    placeholderTextColor={theme.colors.textSecondary}
                                    style={[
                                        styles.input,
                                        emailLocked ? styles.inputDisabled : null,
                                    ]}
                                    textContentType="emailAddress"
                                    value={email}
                                    onChangeText={handleEmailChange}
                                />
                                {emailLocked ? (
                                    <View style={styles.linkRow}>
                                        <Text style={styles.helperText}>We checked this email already.</Text>
                                        <Pressable style={styles.linkButton} disabled={busy} onPress={resetFlow}>
                                            <Text style={styles.linkText}>Use a different email</Text>
                                        </Pressable>
                                    </View>
                                ) : (
                                    <Text style={styles.helperText}>
                                        We&apos;ll check whether this email should log in or create an account.
                                    </Text>
                                )}
                            </View>

                            {step === 'password' ? (
                                <>
                                    <View style={styles.fieldGroup}>
                                        <Text style={styles.label}>Password</Text>
                                        <TextInput
                                            autoCapitalize="none"
                                            autoComplete="current-password"
                                            editable={!busy}
                                            placeholder="Enter your password"
                                            placeholderTextColor={theme.colors.textSecondary}
                                            secureTextEntry
                                            style={styles.input}
                                            textContentType="password"
                                            value={password}
                                            onChangeText={setPassword}
                                        />
                                    </View>

                                    <View style={styles.linkRow}>
                                        <Pressable
                                            style={styles.linkButton}
                                            disabled={busy}
                                            onPress={() => {
                                                void openLandingUrl(buildLandingForgotPasswordUrl());
                                            }}
                                        >
                                            <Text style={styles.linkText}>Forgot password?</Text>
                                        </Pressable>
                                    </View>
                                </>
                            ) : null}

                            {step === 'social' ? (
                                <View style={styles.infoBox}>
                                    <Text style={styles.infoTitle}>Use your social login</Text>
                                    <Text style={styles.infoText}>{socialPrompt}</Text>
                                </View>
                            ) : null}

                            {step === 'register' ? (
                                <>
                                    <View style={styles.fieldGroup}>
                                        <Text style={styles.label}>Create a password</Text>
                                        <TextInput
                                            autoCapitalize="none"
                                            autoComplete="new-password"
                                            editable={!busy}
                                            placeholder="At least 8 characters"
                                            placeholderTextColor={theme.colors.textSecondary}
                                            secureTextEntry
                                            style={styles.input}
                                            textContentType="newPassword"
                                            value={password}
                                            onChangeText={setPassword}
                                        />
                                    </View>

                                    <Pressable
                                        disabled={busy}
                                        style={styles.termsRow}
                                        onPress={() => {
                                            setTermsAccepted((current) => !current);
                                        }}
                                    >
                                        <View style={[styles.checkbox, termsAccepted ? styles.checkboxChecked : null]}>
                                            {termsAccepted ? <View style={styles.checkboxInner} /> : null}
                                        </View>
                                        <View style={styles.termsContent}>
                                            <Text style={styles.termsText}>
                                                I agree to the Terms of Service and Privacy Policy.
                                                {Platform.OS === 'ios' ? ' Required on iOS.' : ''}
                                            </Text>
                                            <View style={styles.termsLinks}>
                                                <Pressable
                                                    style={styles.linkButton}
                                                    disabled={busy}
                                                    onPress={() => {
                                                        void openLandingUrl(buildLandingTermsUrl());
                                                    }}
                                                >
                                                    <Text style={styles.linkText}>Terms</Text>
                                                </Pressable>
                                                <Pressable
                                                    style={styles.linkButton}
                                                    disabled={busy}
                                                    onPress={() => {
                                                        void openLandingUrl(buildLandingPrivacyUrl());
                                                    }}
                                                >
                                                    <Text style={styles.linkText}>Privacy</Text>
                                                </Pressable>
                                            </View>
                                        </View>
                                    </Pressable>
                                </>
                            ) : null}

                            {error ? (
                                <View style={styles.errorBox}>
                                    <Text style={styles.errorText}>{error}</Text>
                                </View>
                            ) : null}

                            {notice ? (
                                <View style={styles.noticeBox}>
                                    <Text style={styles.noticeText}>{notice}</Text>
                                </View>
                            ) : null}

                            {step === 'email' ? (
                                <RoundButton
                                    title="Continue"
                                    size="large"
                                    loading={pendingAction === 'check-email'}
                                    disabled={busy || !email.trim()}
                                    onPress={handleCheckEmail}
                                />
                            ) : null}

                            {step === 'password' ? (
                                <RoundButton
                                    title={buttonTitle}
                                    size="large"
                                    loading={pendingAction === 'login'}
                                    disabled={busy || password.length < 8}
                                    onPress={handlePasswordLogin}
                                />
                            ) : null}

                            {step === 'register' ? (
                                <RoundButton
                                    title="Create account"
                                    size="large"
                                    loading={pendingAction === 'register'}
                                    disabled={busy || password.length < 8 || (Platform.OS === 'ios' && !termsAccepted)}
                                    onPress={handleRegister}
                                />
                            ) : null}

                            <View style={styles.divider} />

                            <Text style={styles.sectionTitle}>Other Login Methods</Text>
                            <View style={styles.providerList}>
                                {Platform.OS === 'ios' ? (
                                    <View
                                        pointerEvents={busy ? 'none' : 'auto'}
                                        style={busy ? styles.appleButtonDisabled : null}
                                    >
                                        <View style={styles.appleButtonWrap}>
                                            <AppleAuthentication.AppleAuthenticationButton
                                                buttonStyle={AppleAuthentication.AppleAuthenticationButtonStyle.BLACK}
                                                buttonType={AppleAuthentication.AppleAuthenticationButtonType.SIGN_IN}
                                                cornerRadius={18}
                                                onPress={handleAppleLogin}
                                                style={styles.appleButton}
                                            />
                                        </View>
                                    </View>
                                ) : null}

                                <Pressable
                                    disabled={busy}
                                    style={({ pressed }) => [
                                        styles.providerButton,
                                        pressed ? styles.providerButtonPressed : null,
                                        busy ? styles.providerButtonDisabled : null,
                                    ]}
                                    onPress={() => {
                                        void handleGoogleLogin();
                                    }}
                                >
                                    <Text style={styles.providerLabel}>
                                        {pendingAction === 'google'
                                            ? 'Opening Google...'
                                            : 'Continue with Google'}
                                    </Text>
                                </Pressable>
                            </View>
                        </View>
                    </View>
                </ScrollView>
            </KeyboardAvoidingView>
        </View>
    );
}
