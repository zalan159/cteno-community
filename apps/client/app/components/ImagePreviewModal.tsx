import * as React from 'react';
import { View, Image as RNImage, Modal, Dimensions, Pressable, Platform } from 'react-native';
import { Gesture, GestureDetector } from 'react-native-gesture-handler';
import Animated, { useSharedValue, useAnimatedStyle, withTiming, runOnJS } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import { Text } from '@/components/StyledText';

interface ImagePreviewModalProps {
    uri: string;
    visible: boolean;
    onClose: () => void;
}

function ImagePreviewModalNative(props: ImagePreviewModalProps) {
    const { width, height } = Dimensions.get('window');
    const imgW = width;
    const imgH = height * 0.85;

    const scale = useSharedValue(1);
    const savedScale = useSharedValue(1);
    const translateX = useSharedValue(0);
    const translateY = useSharedValue(0);
    const savedTranslateX = useSharedValue(0);
    const savedTranslateY = useSharedValue(0);

    // Reset on open
    React.useEffect(() => {
        if (props.visible) {
            scale.value = 1; savedScale.value = 1;
            translateX.value = 0; translateY.value = 0;
            savedTranslateX.value = 0; savedTranslateY.value = 0;
        }
    }, [props.visible]);

    const pinchGesture = Gesture.Pinch()
        .onUpdate((e) => {
            scale.value = Math.max(0.5, Math.min(10, savedScale.value * e.scale));
        })
        .onEnd(() => {
            if (scale.value < 1) {
                scale.value = withTiming(1);
                translateX.value = withTiming(0);
                translateY.value = withTiming(0);
                savedScale.value = 1;
                savedTranslateX.value = 0;
                savedTranslateY.value = 0;
            } else {
                savedScale.value = scale.value;
            }
        });

    const panGesture = Gesture.Pan()
        .minPointers(1)
        .onUpdate((e) => {
            if (savedScale.value > 1) {
                translateX.value = savedTranslateX.value + e.translationX;
                translateY.value = savedTranslateY.value + e.translationY;
            }
        })
        .onEnd(() => {
            savedTranslateX.value = translateX.value;
            savedTranslateY.value = translateY.value;
        });

    const doubleTapGesture = Gesture.Tap()
        .numberOfTaps(2)
        .onStart(() => {
            if (scale.value > 1) {
                scale.value = withTiming(1);
                translateX.value = withTiming(0);
                translateY.value = withTiming(0);
                savedScale.value = 1;
                savedTranslateX.value = 0;
                savedTranslateY.value = 0;
            } else {
                scale.value = withTiming(3);
                savedScale.value = 3;
            }
        });

    const onClose = props.onClose;
    const singleTapGesture = Gesture.Tap()
        .numberOfTaps(1)
        .onStart(() => {
            'worklet';
            if (savedScale.value <= 1) {
                runOnJS(onClose)();
            }
        });

    const tapGesture = Gesture.Exclusive(doubleTapGesture, singleTapGesture);
    const composed = Gesture.Simultaneous(pinchGesture, panGesture, tapGesture);

    const animatedStyle = useAnimatedStyle(() => ({
        transform: [
            { translateX: translateX.value },
            { translateY: translateY.value },
            { scale: scale.value },
        ],
    }));

    return (
        <Modal visible={props.visible} transparent animationType="fade" onRequestClose={props.onClose}>
            <View style={styles.overlay}>
                <Pressable style={styles.closeButton} onPress={props.onClose} hitSlop={20}>
                    <View style={styles.closeCircle}>
                        <Text style={styles.closeText}>✕</Text>
                    </View>
                </Pressable>
                <GestureDetector gesture={composed}>
                    <Animated.View style={[{ width: imgW, height: imgH }, animatedStyle]}>
                        <RNImage
                            source={{ uri: props.uri }}
                            style={{ width: imgW, height: imgH }}
                            resizeMode="contain"
                        />
                    </Animated.View>
                </GestureDetector>
            </View>
        </Modal>
    );
}

function ImagePreviewModalWeb(props: ImagePreviewModalProps) {
    const { width, height } = Dimensions.get('window');
    const [scale, setScale] = React.useState(1);
    const [offset, setOffset] = React.useState({ x: 0, y: 0 });
    const dragging = React.useRef(false);
    const lastPos = React.useRef({ x: 0, y: 0 });

    React.useEffect(() => {
        if (props.visible) { setScale(1); setOffset({ x: 0, y: 0 }); }
    }, [props.visible]);

    const handleWheel = React.useCallback((e: any) => {
        e.preventDefault();
        setScale(s => Math.max(0.5, Math.min(10, s * (e.deltaY < 0 ? 1.15 : 0.87))));
    }, []);

    const handlePointerDown = React.useCallback((e: any) => {
        if (scale > 1) {
            dragging.current = true;
            lastPos.current = { x: e.clientX, y: e.clientY };
        }
    }, [scale]);

    const handlePointerMove = React.useCallback((e: any) => {
        if (!dragging.current) return;
        setOffset(o => ({ x: o.x + e.clientX - lastPos.current.x, y: o.y + e.clientY - lastPos.current.y }));
        lastPos.current = { x: e.clientX, y: e.clientY };
    }, []);

    const handlePointerUp = React.useCallback(() => { dragging.current = false; }, []);

    const handleClick = React.useCallback(() => {
        if (!dragging.current && scale <= 1) props.onClose();
    }, [scale, props.onClose]);

    const handleDoubleClick = React.useCallback(() => {
        if (scale > 1) { setScale(1); setOffset({ x: 0, y: 0 }); }
        else { setScale(3); }
    }, [scale]);

    const imgW = width * 0.9;
    const imgH = height * 0.8;

    return (
        <Modal visible={props.visible} transparent animationType="fade" onRequestClose={props.onClose}>
            <View
                style={styles.overlay}
                // @ts-ignore - web events
                onWheel={handleWheel}
                onPointerDown={handlePointerDown}
                onPointerMove={handlePointerMove}
                onPointerUp={handlePointerUp}
                onClick={handleClick}
                onDoubleClick={handleDoubleClick}
            >
                <Pressable style={styles.closeButton} onPress={props.onClose} hitSlop={20}>
                    <View style={styles.closeCircle}>
                        <Text style={styles.closeText}>✕</Text>
                    </View>
                </Pressable>
                <RNImage
                    source={{ uri: props.uri }}
                    style={{
                        width: imgW,
                        height: imgH,
                        transform: [
                            { translateX: offset.x },
                            { translateY: offset.y },
                            { scale },
                        ],
                    }}
                    resizeMode="contain"
                />
                {scale <= 1 && (
                    <Text style={styles.hint}>scroll to zoom · double-click to 3×</Text>
                )}
            </View>
        </Modal>
    );
}

export const ImagePreviewModal: React.ComponentType<ImagePreviewModalProps> =
    Platform.OS === 'web' ? ImagePreviewModalWeb : ImagePreviewModalNative;

const styles = StyleSheet.create((_theme) => ({
    overlay: {
        flex: 1,
        backgroundColor: 'rgba(0,0,0,0.9)',
        justifyContent: 'center',
        alignItems: 'center',
        cursor: 'default',
        userSelect: 'none',
    } as any,
    closeButton: {
        position: 'absolute',
        top: 54,
        right: 20,
        zIndex: 10,
        padding: 4,
    },
    closeCircle: {
        width: 36,
        height: 36,
        borderRadius: 18,
        backgroundColor: 'rgba(255,255,255,0.2)',
        alignItems: 'center',
        justifyContent: 'center',
    },
    closeText: {
        color: '#fff',
        fontSize: 20,
        lineHeight: 22,
    },
    hint: {
        position: 'absolute',
        bottom: 24,
        color: 'rgba(255,255,255,0.45)',
        fontSize: 13,
    },
}));
