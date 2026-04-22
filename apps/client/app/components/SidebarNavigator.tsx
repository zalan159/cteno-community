import { useAuth } from '@/auth/AuthContext';
import * as React from 'react';
import { Drawer } from 'expo-router/drawer';
import { useIsTablet } from '@/utils/responsive';
import { SidebarView } from './SidebarView';
import { useLocalSettingMutable } from '@/sync/storage';
import { Platform, useWindowDimensions, View } from 'react-native';
import { useUnistyles } from 'react-native-unistyles';

const DEFAULT_DRAWER_WIDTH = 280;
const MIN_DRAWER_WIDTH = 250;
const MAX_DRAWER_WIDTH = 520;
const MIN_MAIN_CONTENT_WIDTH = 520;
const RESIZE_HIT_SLOP = 10;
const RESIZE_CURSOR_CLASS = 'cteno-sidebar-resize-cursor';
const RESIZE_CURSOR_STYLE_ID = 'cteno-sidebar-resize-cursor-style';

function clampDrawerWidth(width: number, windowWidth: number) {
    const maxWidth = Math.max(
        MIN_DRAWER_WIDTH,
        Math.min(
            MAX_DRAWER_WIDTH,
            Math.floor(windowWidth * 0.45),
            windowWidth - MIN_MAIN_CONTENT_WIDTH,
        ),
    );

    return Math.min(Math.max(Math.round(width), MIN_DRAWER_WIDTH), maxWidth);
}

function getDefaultDrawerWidth(windowWidth: number) {
    return clampDrawerWidth(Math.floor(windowWidth * 0.3), windowWidth);
}

function ensureResizeCursorStyle() {
    if (typeof document === 'undefined') {
        return;
    }

    if (document.getElementById(RESIZE_CURSOR_STYLE_ID)) {
        return;
    }

    const style = document.createElement('style');
    style.id = RESIZE_CURSOR_STYLE_ID;
    style.textContent = `
html.${RESIZE_CURSOR_CLASS},
html.${RESIZE_CURSOR_CLASS} body,
html.${RESIZE_CURSOR_CLASS} body * {
    cursor: col-resize !important;
}
`;
    document.head.appendChild(style);
}

export const SidebarNavigator = React.memo(() => {
    const auth = useAuth();
    const { theme } = useUnistyles();
    const isTablet = useIsTablet();
    const showPermanentDrawer = auth.hasAppAccess && isTablet;
    const canResizeSidebar = Platform.OS === 'web' && showPermanentDrawer;
    const { width: windowWidth } = useWindowDimensions();
    const [savedDrawerWidth, setSavedDrawerWidth] = useLocalSettingMutable('desktopSidebarWidth');
    const [draggingDrawerWidth, setDraggingDrawerWidth] = React.useState<number | null>(null);
    const [isResizeHovered, setIsResizeHovered] = React.useState(false);
    const [sidebarEdgeClientX, setSidebarEdgeClientX] = React.useState<number | null>(null);
    const dragStateRef = React.useRef<{ startX: number; startWidth: number } | null>(null);
    const rootRef = React.useRef<any>(null);

    const drawerWidth = React.useMemo(() => {
        if (!showPermanentDrawer) {
            return DEFAULT_DRAWER_WIDTH;
        }

        const baseWidth = draggingDrawerWidth ?? savedDrawerWidth ?? getDefaultDrawerWidth(windowWidth);
        return clampDrawerWidth(baseWidth, windowWidth);
    }, [draggingDrawerWidth, savedDrawerWidth, showPermanentDrawer, windowWidth]);

    React.useEffect(() => {
        if (!showPermanentDrawer || savedDrawerWidth === null) {
            return;
        }

        const clampedWidth = clampDrawerWidth(savedDrawerWidth, windowWidth);
        if (clampedWidth !== savedDrawerWidth) {
            setSavedDrawerWidth(clampedWidth);
        }
    }, [savedDrawerWidth, setSavedDrawerWidth, showPermanentDrawer, windowWidth]);

    const stopResize = React.useCallback(() => {
        if (typeof document !== 'undefined') {
            document.documentElement.classList.remove(RESIZE_CURSOR_CLASS);
            document.body.style.cursor = '';
            document.documentElement.style.cursor = '';
            document.body.style.userSelect = '';
        }

        setDraggingDrawerWidth(null);
        setIsResizeHovered(false);
        dragStateRef.current = null;
    }, []);

    const getDrawerEdgeClientX = React.useCallback(() => {
        if (sidebarEdgeClientX !== null) {
            return sidebarEdgeClientX;
        }

        if (typeof window === 'undefined') {
            return drawerWidth;
        }

        const rootEl = rootRef.current as HTMLElement | null;
        if (!rootEl || typeof rootEl.getBoundingClientRect !== 'function') {
            return drawerWidth;
        }

        return rootEl.getBoundingClientRect().left + drawerWidth;
    }, [drawerWidth, sidebarEdgeClientX]);

    React.useEffect(() => {
        if (!canResizeSidebar) {
            return;
        }

        const handlePointerMove = (event: PointerEvent) => {
            const dragState = dragStateRef.current;
            if (!dragState) {
                return;
            }

            const nextWidth = clampDrawerWidth(
                dragState.startWidth + (event.clientX - dragState.startX),
                windowWidth,
            );
            setDraggingDrawerWidth(nextWidth);
        };

        const handlePointerUp = (event: PointerEvent) => {
            const dragState = dragStateRef.current;
            if (!dragState) {
                return;
            }

            const nextWidth = clampDrawerWidth(
                dragState.startWidth + (event.clientX - dragState.startX),
                windowWidth,
            );
            setSavedDrawerWidth(nextWidth);
            stopResize();
        };

        window.addEventListener('pointermove', handlePointerMove);
        window.addEventListener('pointerup', handlePointerUp);
        window.addEventListener('pointercancel', handlePointerUp);

        return () => {
            window.removeEventListener('pointermove', handlePointerMove);
            window.removeEventListener('pointerup', handlePointerUp);
            window.removeEventListener('pointercancel', handlePointerUp);
            stopResize();
        };
    }, [canResizeSidebar, setSavedDrawerWidth, stopResize, windowWidth]);

    const handleResizeStart = React.useCallback((event: any) => {
        if (!canResizeSidebar) {
            return;
        }

        event.preventDefault?.();
        event.stopPropagation?.();

        dragStateRef.current = {
            startX: event.clientX,
            startWidth: drawerWidth,
        };
        setDraggingDrawerWidth(drawerWidth);

        if (typeof document !== 'undefined') {
            ensureResizeCursorStyle();
            document.documentElement.classList.add(RESIZE_CURSOR_CLASS);
            document.body.style.cursor = 'col-resize';
            document.documentElement.style.cursor = 'col-resize';
            document.body.style.userSelect = 'none';
        }
    }, [canResizeSidebar, drawerWidth]);

    React.useEffect(() => {
        if (!canResizeSidebar) {
            return;
        }

        ensureResizeCursorStyle();

        const setResizeCursor = (active: boolean) => {
            if (typeof document === 'undefined') {
                return;
            }
            document.documentElement.classList.toggle(RESIZE_CURSOR_CLASS, active);
            document.body.style.cursor = active ? 'col-resize' : '';
            document.documentElement.style.cursor = active ? 'col-resize' : '';
        };

        const isNearDrawerEdge = (clientX: number) => {
            const edgeX = getDrawerEdgeClientX();
            return Math.abs(clientX - edgeX) <= RESIZE_HIT_SLOP;
        };

        const handleMouseMove = (event: MouseEvent) => {
            if (dragStateRef.current) {
                setResizeCursor(true);
                return;
            }

            const hovered = isNearDrawerEdge(event.clientX);
            setIsResizeHovered((current) => current === hovered ? current : hovered);
            setResizeCursor(hovered);
        };

        const handleMouseDown = (event: MouseEvent) => {
            if (!isNearDrawerEdge(event.clientX)) {
                return;
            }

            setIsResizeHovered(true);
            handleResizeStart(event);
        };

        window.addEventListener('mousemove', handleMouseMove);
        window.addEventListener('mousedown', handleMouseDown);

        return () => {
            window.removeEventListener('mousemove', handleMouseMove);
            window.removeEventListener('mousedown', handleMouseDown);
            setResizeCursor(false);
        };
    }, [canResizeSidebar, getDrawerEdgeClientX, handleResizeStart]);

    // Always use 'permanent' drawerType to prevent remounting the Stack
    // when switching between sidebar and normal mode (which resets navigation state).
    // Instead, toggle visibility by setting width to 0.
    const drawerNavigationOptions = React.useMemo(() => ({
        lazy: false,
        headerShown: false,
        drawerType: 'permanent' as const,
        drawerStyle: {
            backgroundColor: 'white',
            borderRightWidth: 0,
            width: showPermanentDrawer ? drawerWidth : 0,
            display: (showPermanentDrawer ? 'flex' : 'none') as any,
        },
        swipeEnabled: false,
        drawerActiveTintColor: 'transparent',
        drawerInactiveTintColor: 'transparent',
        drawerItemStyle: { display: 'none' as const },
        drawerLabelStyle: { display: 'none' as const },
    }), [showPermanentDrawer, drawerWidth]);

    const drawerContent = React.useCallback(
        () => (
            <SidebarView
                sidebarWidth={drawerWidth}
                showResizeHandle={draggingDrawerWidth !== null}
                isResizing={draggingDrawerWidth !== null}
                onEdgeChange={(clientRight) => {
                    setSidebarEdgeClientX((current) => {
                        if (current !== null && Math.abs(current - clientRight) < 0.5) {
                            return current;
                        }
                        return clientRight;
                    });
                }}
            />
        ),
        [drawerWidth, draggingDrawerWidth]
    );

    return (
        <View ref={rootRef} style={{ flex: 1, position: 'relative' }}>
            <Drawer
                screenOptions={drawerNavigationOptions}
                drawerContent={drawerContent}
            />
        </View>
    )
});
