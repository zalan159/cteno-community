/**
 * OrchestrationFlowView
 *
 * Renders an orchestration flow graph: SVG edges + native RN node views.
 * Reuses the topological layering approach from DispatchTaskView.
 */
import * as React from 'react';
import { View, Pressable, Platform, ActivityIndicator } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { Ionicons } from '@expo/vector-icons';
import Svg, { Path, Defs, Marker, Polygon } from 'react-native-svg';
import { Text } from '@/components/StyledText';
import { useNavigateToSession } from '@/hooks/useNavigateToSession';
import type { OrchestrationFlow, FlowNode, FlowEdge, FlowNodeStatus } from '@/sync/storageTypes';

// ─── Layout constants ────────────────────────────────────────────────
const NODE_W = 160;
const NODE_H = 50;
const GAP_X = 20;
const GAP_Y = 44;
const PAD = 16;

// ─── Status colors ───────────────────────────────────────────────────
const STATUS_COLORS: Record<FlowNodeStatus, string> = {
    pending: '#8E8E93',
    running: '#007AFF',
    completed: '#34C759',
    failed: '#FF3B30',
    skipped: '#C7C7CC',
};

const STATUS_ICONS: Record<FlowNodeStatus, string> = {
    pending: 'ellipse-outline',
    running: 'play-circle',
    completed: 'checkmark-circle',
    failed: 'close-circle',
    skipped: 'remove-circle-outline',
};

// ─── Topological layering (forward edges only) ──────────────────────

interface NodePos { x: number; y: number; cx: number; topY: number; botY: number }

function computeLayers(nodes: FlowNode[], edges: FlowEdge[]): FlowNode[][] {
    // Only use forward edges (normal + conditional) for layering
    const forwardEdges = edges.filter(e => e.edgeType !== 'retry');
    const layers: FlowNode[][] = [];
    const placed = new Set<string>();

    while (placed.size < nodes.length) {
        const layer: FlowNode[] = [];
        for (const node of nodes) {
            if (placed.has(node.id)) continue;
            // Check if all dependencies (sources of edges ending at this node) are placed
            const deps = forwardEdges
                .filter(e => e.to === node.id)
                .map(e => e.from);
            if (deps.every(d => placed.has(d))) {
                layer.push(node);
            }
        }
        if (layer.length === 0) {
            // Remaining nodes have circular deps; place them all
            for (const node of nodes) {
                if (!placed.has(node.id)) layer.push(node);
            }
            for (const n of layer) placed.add(n.id);
            layers.push(layer);
            break;
        }
        for (const n of layer) placed.add(n.id);
        layers.push(layer);
    }

    return layers;
}

function buildLayout(layers: FlowNode[][]): Map<string, NodePos> {
    const pos = new Map<string, NodePos>();

    for (let li = 0; li < layers.length; li++) {
        const layer = layers[li];
        let x = PAD;
        const y = PAD + li * (NODE_H + GAP_Y);

        for (const node of layer) {
            pos.set(node.id, {
                x, y,
                cx: x + NODE_W / 2,
                topY: y,
                botY: y + NODE_H,
            });
            x += NODE_W + GAP_X;
        }
    }

    return pos;
}

// ─── Component ───────────────────────────────────────────────────────

interface OrchestrationFlowViewProps {
    flow: OrchestrationFlow;
    compact?: boolean;
}

export function OrchestrationFlowView({ flow, compact = false }: OrchestrationFlowViewProps) {
    const navigateToSession = useNavigateToSession();

    const { nodes, edges } = flow;
    if (nodes.length === 0) return null;

    const layers = computeLayers(nodes, edges);
    const positions = buildLayout(layers);

    // Canvas size
    let maxX = 0, maxY = 0;
    positions.forEach(p => {
        maxX = Math.max(maxX, p.x + NODE_W);
        maxY = Math.max(maxY, p.y + NODE_H);
    });
    const canvasW = maxX + PAD;
    const canvasH = maxY + PAD;

    // Separate forward edges and back (retry) edges
    const forwardEdges: { from: NodePos; to: NodePos; edge: FlowEdge }[] = [];
    const retryEdges: { from: NodePos; to: NodePos; edge: FlowEdge }[] = [];

    for (const edge of edges) {
        const fromPos = positions.get(edge.from);
        const toPos = positions.get(edge.to);
        if (!fromPos || !toPos) continue;

        if (edge.edgeType === 'retry') {
            retryEdges.push({ from: fromPos, to: toPos, edge });
        } else {
            forwardEdges.push({ from: fromPos, to: toPos, edge });
        }
    }

    // Progress summary
    const completedCount = nodes.filter(n => n.status === 'completed').length;
    const totalCount = nodes.length;

    return (
        <View style={styles.container}>
            {/* Progress header */}
            <View style={styles.header}>
                <Text style={styles.title} numberOfLines={1}>{flow.title}</Text>
                <View style={styles.progressPill}>
                    <Text style={styles.progressText}>
                        {completedCount}/{totalCount}
                    </Text>
                </View>
            </View>

            {/* Flow canvas */}
            <View style={{ width: canvasW, height: canvasH }}>
                {/* SVG layer: edges */}
                <Svg
                    width={canvasW}
                    height={canvasH}
                    style={{ position: 'absolute', left: 0, top: 0 }}
                >
                    <Defs>
                        <Marker
                            id="orch-arrow"
                            markerWidth="8"
                            markerHeight="6"
                            refX="8"
                            refY="3"
                            orient="auto"
                        >
                            <Polygon points="0,0 8,3 0,6" fill="#8E8E93" opacity={0.6} />
                        </Marker>
                        <Marker
                            id="orch-retry-arrow"
                            markerWidth="8"
                            markerHeight="6"
                            refX="8"
                            refY="3"
                            orient="auto"
                        >
                            <Polygon points="0,0 8,3 0,6" fill="#FF9500" opacity={0.6} />
                        </Marker>
                    </Defs>

                    {/* Forward edges: bezier from bottom of source to top of target */}
                    {forwardEdges.map((e, i) => {
                        const x1 = e.from.cx;
                        const y1 = e.from.botY;
                        const x2 = e.to.cx;
                        const y2 = e.to.topY;
                        const dy = (y2 - y1) * 0.5;

                        const isConditional = e.edge.edgeType === 'conditional';
                        const strokeColor = isConditional ? '#FF9500' : '#8E8E93';

                        return (
                            <Path
                                key={`fwd-${i}`}
                                d={`M${x1},${y1} C${x1},${y1 + dy} ${x2},${y2 - dy} ${x2},${y2}`}
                                stroke={strokeColor}
                                strokeWidth={1.5}
                                fill="none"
                                opacity={0.5}
                                markerEnd="url(#orch-arrow)"
                                strokeDasharray={isConditional ? "6,3" : undefined}
                            />
                        );
                    })}

                    {/* Retry edges: right-side curved path from target back to source */}
                    {retryEdges.map((e, i) => {
                        const rightOffset = canvasW - PAD + 20;
                        const fromMidY = e.from.topY + NODE_H / 2;
                        const toMidY = e.to.topY + NODE_H / 2;
                        const fromRightX = e.from.cx + NODE_W / 2;
                        const toRightX = e.to.cx + NODE_W / 2;

                        return (
                            <Path
                                key={`retry-${i}`}
                                d={`M${fromRightX},${fromMidY} C${rightOffset},${fromMidY} ${rightOffset},${toMidY} ${toRightX},${toMidY}`}
                                stroke="#FF9500"
                                strokeWidth={1.5}
                                fill="none"
                                opacity={0.4}
                                strokeDasharray="6,3"
                                markerEnd="url(#orch-retry-arrow)"
                            />
                        );
                    })}
                </Svg>

                {/* RN layer: nodes */}
                {nodes.map(node => {
                    const p = positions.get(node.id);
                    if (!p) return null;
                    const color = STATUS_COLORS[node.status];
                    const iconName = STATUS_ICONS[node.status] as any;
                    const canNavigate = !!node.sessionId;

                    return (
                        <Pressable
                            key={node.id}
                            disabled={!canNavigate}
                            onPress={() => {
                                if (node.sessionId) {
                                    navigateToSession(node.sessionId);
                                }
                            }}
                            style={[
                                nodeStyles.node,
                                {
                                    position: 'absolute',
                                    left: p.x,
                                    top: p.y,
                                    width: NODE_W,
                                    height: NODE_H,
                                    borderColor: color + '40',
                                    backgroundColor: color + '10',
                                },
                            ]}
                        >
                            {node.status === 'running' ? (
                                <ActivityIndicator size="small" color={color} style={{ marginRight: 6 }} />
                            ) : (
                                <Ionicons name={iconName} size={16} color={color} style={{ marginRight: 6 }} />
                            )}
                            <View style={{ flex: 1 }}>
                                <Text
                                    style={[nodeStyles.label, { color }]}
                                    numberOfLines={1}
                                >
                                    {node.label}
                                </Text>
                                {node.iteration != null && node.maxIterations != null && (
                                    <Text style={[nodeStyles.iterationText, { color: color + '99' }]}>
                                        iter {node.iteration}/{node.maxIterations}
                                    </Text>
                                )}
                            </View>
                            {canNavigate && (
                                <Ionicons name="chevron-forward" size={14} color={color + '60'} />
                            )}
                        </Pressable>
                    );
                })}
            </View>
        </View>
    );
}

// ─── Compact version for BackgroundRunsModal ─────────────────────────

interface OrchestrationFlowCompactProps {
    flow: OrchestrationFlow;
}

export function OrchestrationFlowCompact({ flow }: OrchestrationFlowCompactProps) {
    const completedCount = flow.nodes.filter(n => n.status === 'completed').length;
    const runningCount = flow.nodes.filter(n => n.status === 'running').length;
    const failedCount = flow.nodes.filter(n => n.status === 'failed').length;
    const totalCount = flow.nodes.length;

    return (
        <View style={compactStyles.container}>
            <View style={compactStyles.row}>
                <Ionicons name="git-network-outline" size={16} color="#007AFF" />
                <Text style={compactStyles.title} numberOfLines={1}>{flow.title}</Text>
            </View>
            <View style={compactStyles.row}>
                <Text style={compactStyles.stat}>
                    {completedCount}/{totalCount} completed
                </Text>
                {runningCount > 0 && (
                    <Text style={[compactStyles.stat, { color: '#007AFF' }]}>
                        {runningCount} running
                    </Text>
                )}
                {failedCount > 0 && (
                    <Text style={[compactStyles.stat, { color: '#FF3B30' }]}>
                        {failedCount} failed
                    </Text>
                )}
            </View>
            {/* Mini node status bar */}
            <View style={compactStyles.statusBar}>
                {flow.nodes.map(node => (
                    <View
                        key={node.id}
                        style={[
                            compactStyles.statusDot,
                            { backgroundColor: STATUS_COLORS[node.status] },
                        ]}
                    />
                ))}
            </View>
        </View>
    );
}

// ─── Styles ──────────────────────────────────────────────────────────

const styles = StyleSheet.create((theme) => ({
    container: {
        paddingHorizontal: 12,
        paddingVertical: 8,
        gap: 8,
    },
    header: {
        flexDirection: 'row',
        alignItems: 'center',
        justifyContent: 'space-between',
        gap: 8,
    },
    title: {
        fontSize: 14,
        fontWeight: '600',
        color: theme.colors.text,
        flex: 1,
    },
    progressPill: {
        backgroundColor: theme.colors.surfaceHigh,
        paddingHorizontal: 10,
        paddingVertical: 3,
        borderRadius: 12,
    },
    progressText: {
        fontSize: 12,
        fontWeight: '600',
        color: theme.colors.textSecondary,
    },
}));

const nodeStyles = StyleSheet.create((_theme) => ({
    node: {
        flexDirection: 'row',
        alignItems: 'center',
        borderRadius: 8,
        borderWidth: 1,
        paddingHorizontal: 10,
    },
    label: {
        fontSize: 13,
        fontWeight: '600',
        fontFamily: Platform.select({ ios: 'Menlo', default: 'monospace' }),
    },
    iterationText: {
        fontSize: 10,
        marginTop: 1,
    },
}));

const compactStyles = StyleSheet.create((theme) => ({
    container: {
        gap: 6,
        paddingVertical: 8,
        paddingHorizontal: 12,
        backgroundColor: theme.colors.surfaceHigh,
        borderRadius: 10,
    },
    row: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 8,
    },
    title: {
        fontSize: 14,
        fontWeight: '600',
        color: theme.colors.text,
        flex: 1,
    },
    stat: {
        fontSize: 12,
        color: theme.colors.textSecondary,
    },
    statusBar: {
        flexDirection: 'row',
        gap: 3,
        marginTop: 2,
    },
    statusDot: {
        width: 8,
        height: 8,
        borderRadius: 4,
    },
}));
