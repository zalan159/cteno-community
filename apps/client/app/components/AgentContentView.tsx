import { useHeaderHeight } from '@/utils/responsive';
import * as React from 'react';
import { View } from 'react-native';
import { ScrollView } from 'react-native-gesture-handler';
import { useKeyboardState } from 'react-native-keyboard-controller';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

interface AgentContentViewProps {
    input?: React.ReactNode | null;
    content?: React.ReactNode | null;
    placeholder?: React.ReactNode | null;
}

export const AgentContentView: React.FC<AgentContentViewProps> = React.memo(({ input, content, placeholder }) => {
    const safeArea = useSafeAreaInsets();
    const headerHeight = useHeaderHeight();
    const state = useKeyboardState();
    return (
        <View style={{ flexBasis:0, flexGrow:1, minHeight: 0, paddingBottom: state.isVisible ? state.height - safeArea.bottom : 0 }}>
            <View style={{ flexBasis:0, flexGrow:1, minHeight: 0 }}>
                {content ? (
                    <View style={{ flex: 1, minHeight: 0 }}>
                        {content}
                    </View>
                ) : placeholder ? (
                    <ScrollView
                        style={{ flex: 1, minHeight: 0 }}
                        contentContainerStyle={{
                            alignItems: 'center',
                            justifyContent: 'center',
                            flexGrow: 1,
                            paddingTop: safeArea.top + headerHeight,
                        }}
                        keyboardShouldPersistTaps="handled"
                        alwaysBounceVertical={false}
                    >
                        {placeholder}
                    </ScrollView>
                ) : null}
            </View>
            <View>
                {input}
            </View>
        </View>
    );
});

// const FallbackKeyboardAvoidingView: React.FC<AgentContentViewProps> = React.memo(({
//     children,
// }) => {
    
// });
