import React, { useEffect, useMemo, useRef, useState } from 'react';
import { View, LayoutChangeEvent } from 'react-native';
import Svg, { Path, G } from 'react-native-svg';
import { useUnistyles } from 'react-native-unistyles';

interface Wave {
    id: number;
    baseYRatio: number;
    amplitude: number;
    frequency: number;
    speed: number;
    phase: number;
    color: string;
    opacity: number;
    strokeWidth: number;
    pulseSpeed: number;
    pulseDepth: number;
    harmonic: number;
    harmonicAmp: number;
}

const WAVE_COLORS_DARK = [
    '#FF6B6B',
    '#4ECDC4',
    '#45B7D1',
    '#A78BFA',
    '#F9A8D4',
    '#34D399',
    '#FBBF24',
    '#818CF8',
];

const WAVE_COLORS_LIGHT = [
    '#E74C3C',
    '#1ABC9C',
    '#2E86C1',
    '#8E44AD',
    '#E91E63',
    '#27AE60',
    '#F39C12',
    '#5B5EA6',
];

const WAVE_COUNT = 7;

function makeWaves(isDark: boolean): Wave[] {
    const colors = isDark ? WAVE_COLORS_DARK : WAVE_COLORS_LIGHT;
    return Array.from({ length: WAVE_COUNT }, (_, i) => ({
        id: i,
        baseYRatio: 0.15 + 0.7 * (i / (WAVE_COUNT - 1)),
        amplitude: 15 + Math.random() * 30,
        frequency: 0.8 + Math.random() * 1.5,
        speed: (0.3 + Math.random() * 0.6) * (Math.random() > 0.5 ? 1 : -1),
        phase: Math.random() * Math.PI * 2,
        color: colors[i % colors.length],
        opacity: isDark ? 0.35 + Math.random() * 0.25 : 0.25 + Math.random() * 0.2,
        strokeWidth: 1.5 + Math.random() * 1.5,
        pulseSpeed: 0.2 + Math.random() * 0.4,
        pulseDepth: 0.3 + Math.random() * 0.4,
        harmonic: 2 + Math.floor(Math.random() * 3),
        harmonicAmp: 0.15 + Math.random() * 0.25,
    }));
}

export function AnimatedGeometricBackground() {
    const { theme } = useUnistyles();
    const [size, setSize] = useState({ w: 0, h: 0 });

    const onLayout = (e: LayoutChangeEvent) => {
        const { width, height } = e.nativeEvent.layout;
        setSize(prev => (prev.w === width && prev.h === height) ? prev : { w: width, h: height });
    };

    const waves = useMemo(() => makeWaves(theme.dark), [theme.dark]);

    const [, forceUpdate] = React.useReducer(x => x + 1, 0);
    const startTime = useRef(Date.now());

    useEffect(() => {
        let animationFrame: number;
        let lastTime = 0;
        const interval = 1000 / 30;

        const animate = (currentTime: number) => {
            const delta = currentTime - lastTime;
            if (delta >= interval) {
                forceUpdate();
                lastTime = currentTime - (delta % interval);
            }
            animationFrame = requestAnimationFrame(animate);
        };

        animationFrame = requestAnimationFrame(animate);
        return () => cancelAnimationFrame(animationFrame);
    }, []);

    const { w, h } = size;
    const t = (Date.now() - startTime.current) / 1000;

    const buildPath = (wave: Wave): string => {
        if (w === 0) return '';
        const step = 4;
        const pulse = 1 - wave.pulseDepth + wave.pulseDepth * Math.sin(t * wave.pulseSpeed * Math.PI * 2);
        const amp = wave.amplitude * pulse;
        const baseY = wave.baseYRatio * h;
        const parts: string[] = [];

        for (let x = 0; x <= w; x += step) {
            const nx = x / w;
            const main = Math.sin(nx * wave.frequency * Math.PI * 2 + t * wave.speed + wave.phase);
            const harm = Math.sin(nx * wave.frequency * wave.harmonic * Math.PI * 2 + t * wave.speed * 1.3 + wave.phase * 2);
            const y = baseY + amp * (main + wave.harmonicAmp * harm);
            parts.push(x === 0 ? `M0 ${y.toFixed(1)}` : `L${x} ${y.toFixed(1)}`);
        }

        return parts.join(' ');
    };

    return (
        <View
            onLayout={onLayout}
            style={{ position: 'absolute', top: 0, left: 0, right: 0, bottom: 0, zIndex: 0 }}
            pointerEvents="none"
        >
            {w > 0 && (
                <Svg width={w} height={h}>
                    <G>
                        {waves.map((wave) => {
                            const d = buildPath(wave);
                            return (
                                <G key={wave.id}>
                                    <Path
                                        d={d}
                                        stroke={wave.color}
                                        strokeWidth={wave.strokeWidth * 4}
                                        fill="none"
                                        opacity={wave.opacity * 0.25}
                                        strokeLinecap="round"
                                        strokeLinejoin="round"
                                    />
                                    <Path
                                        d={d}
                                        stroke={wave.color}
                                        strokeWidth={wave.strokeWidth}
                                        fill="none"
                                        opacity={wave.opacity}
                                        strokeLinecap="round"
                                        strokeLinejoin="round"
                                    />
                                </G>
                            );
                        })}
                    </G>
                </Svg>
            )}
        </View>
    );
}
