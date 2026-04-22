import * as React from "react";
import { View } from "react-native";
import { Image } from "expo-image";
import { AvatarSkia } from "./AvatarSkia";
import { AvatarGradient } from "./AvatarGradient";
import { AvatarBrutalist } from "./AvatarBrutalist";
import { useSetting } from '@/sync/storage';
import { StyleSheet } from 'react-native-unistyles';
import { isModelAvatar, MODEL_AVATAR_IMAGES } from '@/utils/modelAvatars';
import { getVendorFromAvatarId, getVendorIconSource, isVendorAvatarId } from '@/utils/vendorIcons';

interface AvatarProps {
    id: string;
    title?: boolean;
    square?: boolean;
    size?: number;
    monochrome?: boolean;
    flavor?: string | null;
    imageUrl?: string | null;
    thumbhash?: string | null;
}

const styles = StyleSheet.create((theme) => ({
    container: {
        position: 'relative',
    },
    flavorIcon: {
        position: 'absolute',
        bottom: -2,
        right: -2,
        backgroundColor: theme.colors.surface,
        borderRadius: 100,
        padding: 2,
        shadowColor: theme.colors.shadow.color,
        shadowOffset: { width: 0, height: 1 },
        shadowOpacity: 0.2,
        shadowRadius: 2,
        elevation: 3,
    },
}));

export const Avatar = React.memo((props: AvatarProps) => {
    const { flavor, size = 48, imageUrl, thumbhash, ...avatarProps } = props;
    const avatarStyle = useSetting('avatarStyle');
    const showFlavorIcons = useSetting('showFlavorIcons');

    const effectiveFlavor = flavor || 'cteno';
    const shouldShowFlavorBadge = showFlavorIcons
        && !!flavor
        && effectiveFlavor !== 'claude'
        && effectiveFlavor !== 'codex'
        && effectiveFlavor !== 'gemini';
    const flavorIcon = getVendorIconSource(effectiveFlavor);
    const circleSize = Math.round(size * 0.35);
    const iconSize = effectiveFlavor === 'codex'
        ? Math.round(size * 0.25)
        : effectiveFlavor === 'claude'
            ? Math.round(size * 0.28)
            : effectiveFlavor === 'cteno'
                ? Math.round(size * 0.3)
                : Math.round(size * 0.35);

    // Render model avatar PNG if the id matches a known model
    if (isModelAvatar(props.id)) {
        const modelImage = (
            <Image
                source={{ uri: MODEL_AVATAR_IMAGES[props.id] }}
                contentFit="cover"
                style={{
                    width: size,
                    height: size,
                    borderRadius: props.square ? 0 : size / 2,
                }}
            />
        );

        if (shouldShowFlavorBadge) {
            return (
                <View style={[styles.container, { width: size, height: size }]}>
                    {modelImage}
                    <View style={[styles.flavorIcon, {
                        width: circleSize,
                        height: circleSize,
                        alignItems: 'center',
                        justifyContent: 'center',
                    }]}>
                        <Image
                            source={flavorIcon}
                            style={{ width: iconSize, height: iconSize }}
                            contentFit="contain"
                        />
                    </View>
                </View>
            );
        }

        return modelImage;
    }

    if (isVendorAvatarId(props.id)) {
        const vendorFromId = getVendorFromAvatarId(props.id) || effectiveFlavor;
        const vendorSource = getVendorIconSource(vendorFromId);
        return (
            <Image
                source={vendorSource}
                contentFit="cover"
                style={{
                    width: size,
                    height: size,
                    borderRadius: props.square ? 0 : size / 2,
                }}
            />
        );
    }

    // Render custom image if provided
    if (imageUrl) {
        const imageElement = (
            <Image
                source={{ uri: imageUrl, thumbhash: thumbhash || undefined }}
                placeholder={thumbhash ? { thumbhash: thumbhash } : undefined}
                contentFit="cover"
                style={{
                    width: size,
                    height: size,
                    borderRadius: avatarProps.square ? 0 : size / 2
                }}
            />
        );

        // Add flavor icon overlay if enabled
        if (shouldShowFlavorBadge) {
            return (
                <View style={[styles.container, { width: size, height: size }]}>
                    {imageElement}
                    <View style={[styles.flavorIcon, {
                        width: circleSize,
                        height: circleSize,
                        alignItems: 'center',
                        justifyContent: 'center'
                    }]}>
                        <Image
                            source={flavorIcon}
                            style={{ width: iconSize, height: iconSize }}
                            contentFit="contain"
                        />
                    </View>
                </View>
            );
        }

        return imageElement;
    }

    // Original generated avatar logic
    // Determine which avatar variant to render
    let AvatarComponent: React.ComponentType<any>;
    if (avatarStyle === 'pixelated') {
        AvatarComponent = AvatarSkia;
    } else if (avatarStyle === 'brutalist') {
        AvatarComponent = AvatarBrutalist;
    } else {
        AvatarComponent = AvatarGradient;
    }

    // Only wrap in container if showing flavor icons
    if (shouldShowFlavorBadge) {
        return (
            <View style={[styles.container, { width: size, height: size }]}>
                <AvatarComponent {...avatarProps} size={size} />
                <View style={[styles.flavorIcon, {
                    width: circleSize,
                    height: circleSize,
                    alignItems: 'center',
                    justifyContent: 'center'
                }]}>
                    <Image
                        source={flavorIcon}
                        style={{ width: iconSize, height: iconSize }}
                        contentFit="contain"
                    />
                </View>
            </View>
        );
    }

    // Return avatar without wrapper when not showing flavor icons
    return <AvatarComponent {...avatarProps} size={size} />;
});
