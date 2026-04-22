import * as React from 'react';
import { useUnistyles } from 'react-native-unistyles';

/**
 * Custom scrollbar for inverted FlatList on web.
 * The native scrollbar's drag direction is flipped by scaleY(-1),
 * so we hide it and render our own with correct behavior.
 *
 * Position mapping (inverted FlatList):
 *   scrollTop=0       → newest messages (visual bottom) → thumb at bottom
 *   scrollTop=max     → oldest messages (visual top)    → thumb at top
 */
export function InvertedScrollbar(props: { containerRef: React.RefObject<HTMLDivElement | null> }) {
    const { theme } = useUnistyles();
    const trackRef = React.useRef<HTMLDivElement>(null);
    const thumbRef = React.useRef<HTMLDivElement>(null);
    const [thumbHeight, setThumbHeight] = React.useState(0);
    const [thumbTop, setThumbTop] = React.useState(0);
    const [visible, setVisible] = React.useState(false);
    const [hovered, setHovered] = React.useState(false);
    const [dragging, setDragging] = React.useState(false);
    const dragStartRef = React.useRef({ y: 0, scrollTop: 0 });
    const hideTimerRef = React.useRef<ReturnType<typeof setTimeout> | null>(null);
    const scrollElRef = React.useRef<HTMLElement | null>(null);

    // Find the actual scrollable element inside the FlatList container
    const getScrollEl = React.useCallback((): HTMLElement | null => {
        if (scrollElRef.current) return scrollElRef.current;
        const container = props.containerRef.current;
        if (!container) return null;
        // RNW FlatList: container > div[style*=overflow] (the ScrollView)
        const el = container.querySelector('[class*="r-overflow"]') as HTMLElement
            || container.querySelector('[style*="overflow"]') as HTMLElement;
        if (el && (el.scrollHeight > el.clientHeight)) {
            scrollElRef.current = el;
            return el;
        }
        // Fallback: find deepest scrollable element
        const all = container.querySelectorAll('*');
        for (let i = 0; i < all.length; i++) {
            const node = all[i] as HTMLElement;
            if (node.scrollHeight > node.clientHeight && getComputedStyle(node).overflowY !== 'visible' && getComputedStyle(node).overflowY !== 'hidden') {
                scrollElRef.current = node;
                return node;
            }
        }
        return null;
    }, [props.containerRef]);

    const updateThumb = React.useCallback(() => {
        const scrollEl = getScrollEl();
        const track = trackRef.current;
        if (!scrollEl || !track) return;

        const { scrollTop, scrollHeight, clientHeight } = scrollEl;
        if (scrollHeight <= clientHeight) {
            setVisible(false);
            return;
        }

        setVisible(true);
        const trackHeight = track.clientHeight;
        const ratio = clientHeight / scrollHeight;
        const newThumbHeight = Math.max(30, ratio * trackHeight);
        const maxScroll = scrollHeight - clientHeight;
        const availableTrack = trackHeight - newThumbHeight;

        // Invert: scrollTop=0 (newest) → thumb at bottom, scrollTop=max (oldest) → thumb at top
        const newThumbTop = (1 - scrollTop / maxScroll) * availableTrack;

        setThumbHeight(newThumbHeight);
        setThumbTop(Math.max(0, Math.min(availableTrack, newThumbTop)));
    }, [getScrollEl]);

    // Observe scroll events
    React.useEffect(() => {
        // Reset cached scroll element when container changes
        scrollElRef.current = null;

        let rafId: number;
        const poll = () => {
            const el = getScrollEl();
            if (el) {
                const handler = () => {
                    updateThumb();
                    showTemporarily();
                };
                el.addEventListener('scroll', handler, { passive: true });

                // Also observe resize
                const ro = new ResizeObserver(() => updateThumb());
                ro.observe(el);

                // Initial update
                updateThumb();

                return () => {
                    el.removeEventListener('scroll', handler);
                    ro.disconnect();
                };
            }
            // Scroll element not ready yet, retry
            rafId = requestAnimationFrame(poll);
            return undefined;
        };

        const cleanup = poll();
        return () => {
            if (cleanup) cleanup();
            cancelAnimationFrame(rafId);
        };
    }, [getScrollEl, updateThumb]);

    // Also re-detect scroll element when messages change (content updates)
    React.useEffect(() => {
        scrollElRef.current = null;
        const timer = setTimeout(() => updateThumb(), 100);
        return () => clearTimeout(timer);
    }, [updateThumb]);

    const showTemporarily = React.useCallback(() => {
        if (hideTimerRef.current) clearTimeout(hideTimerRef.current);
        setVisible(true);
        if (!dragging && !hovered) {
            hideTimerRef.current = setTimeout(() => setVisible(false), 1500);
        }
    }, [dragging, hovered]);

    // Drag handling
    const handleMouseDown = React.useCallback((e: React.MouseEvent) => {
        e.preventDefault();
        e.stopPropagation();
        const scrollEl = getScrollEl();
        if (!scrollEl) return;

        setDragging(true);
        dragStartRef.current = { y: e.clientY, scrollTop: scrollEl.scrollTop };

        const handleMouseMove = (ev: MouseEvent) => {
            const track = trackRef.current;
            const scrollElement = getScrollEl();
            if (!track || !scrollElement) return;

            const deltaY = ev.clientY - dragStartRef.current.y;
            const trackHeight = track.clientHeight;
            const availableTrack = trackHeight - thumbHeight;
            const maxScroll = scrollElement.scrollHeight - scrollElement.clientHeight;

            // Invert: dragging down should decrease scrollTop (show newer messages)
            const scrollDelta = -(deltaY / availableTrack) * maxScroll;
            scrollElement.scrollTop = dragStartRef.current.scrollTop + scrollDelta;
        };

        const handleMouseUp = () => {
            setDragging(false);
            document.removeEventListener('mousemove', handleMouseMove);
            document.removeEventListener('mouseup', handleMouseUp);
        };

        document.addEventListener('mousemove', handleMouseMove);
        document.addEventListener('mouseup', handleMouseUp);
    }, [getScrollEl, thumbHeight]);

    // Click on track to jump
    const handleTrackClick = React.useCallback((e: React.MouseEvent) => {
        const track = trackRef.current;
        const scrollEl = getScrollEl();
        if (!track || !scrollEl || e.target !== track) return;

        const rect = track.getBoundingClientRect();
        const clickY = e.clientY - rect.top;
        const trackHeight = track.clientHeight;
        const availableTrack = trackHeight - thumbHeight;
        const maxScroll = scrollEl.scrollHeight - scrollEl.clientHeight;

        // Invert: click position maps inversely to scrollTop
        const ratio = clickY / availableTrack;
        scrollEl.scrollTop = (1 - ratio) * maxScroll;
    }, [getScrollEl, thumbHeight]);

    const opacity = (visible || hovered || dragging) ? 1 : 0;

    return (
        <div
            ref={trackRef}
            onClick={handleTrackClick}
            onMouseEnter={() => { setHovered(true); setVisible(true); }}
            onMouseLeave={() => { setHovered(false); showTemporarily(); }}
            style={{
                position: 'absolute',
                top: 0,
                right: 0,
                bottom: 0,
                width: 12,
                zIndex: 10,
                opacity,
                transition: 'opacity 0.3s',
                cursor: 'default',
            }}
        >
            <div
                ref={thumbRef}
                onMouseDown={handleMouseDown}
                style={{
                    position: 'absolute',
                    top: thumbTop,
                    right: 2,
                    width: 8,
                    height: thumbHeight,
                    borderRadius: 4,
                    backgroundColor: (hovered || dragging) ? theme.colors.textSecondary : theme.colors.divider,
                    transition: dragging ? 'none' : 'background-color 0.2s',
                    cursor: 'pointer',
                }}
            />
        </div>
    );
}
