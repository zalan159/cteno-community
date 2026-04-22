import * as React from 'react';
import { View, Platform } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { Ionicons, Octicons } from '@expo/vector-icons';
import Svg, { Path, Defs, Marker, Polygon } from 'react-native-svg';
import type { ToolViewProps } from './_all';
import { Text } from '@/components/StyledText';

interface TaskNode {
    id: string;
    task: string;
    depends_on: string[];
    profile_id?: string;
    skill_ids?: string[];
}

interface GraphResult {
    group_id: string;
    total_tasks: number;
    root_tasks_started: number;
    message: string;
}

interface SingleResult {
    session_id: string;
    message: string;
}

// Topological layering
function computeLayers(tasks: TaskNode[]): TaskNode[][] {
    const layers: TaskNode[][] = [];
    const placed = new Set<string>();

    while (placed.size < tasks.length) {
        const layer: TaskNode[] = [];
        for (const task of tasks) {
            if (placed.has(task.id)) continue;
            const deps = task.depends_on || [];
            if (deps.every(d => placed.has(d))) layer.push(task);
        }
        if (layer.length === 0) break;
        for (const t of layer) placed.add(t.id);
        layers.push(layer);
    }

    return layers;
}

// Layout constants
const NODE_W = 120;
const NODE_H = 30;
const GAP_X = 16;
const GAP_Y = 36;
const PAD = 12;

const C_ROOT = '#007AFF';
const C_DOWN = '#8E8E93';
const C_EDGE = '#8E8E93';

interface NodePos { x: number; y: number; cx: number; topY: number; botY: number }

function buildLayout(layers: TaskNode[][]) {
    const pos = new Map<string, NodePos>();

    for (let li = 0; li < layers.length; li++) {
        const layer = layers[li];
        let x = PAD;
        const y = PAD + li * (NODE_H + GAP_Y);

        for (const task of layer) {
            pos.set(task.id, {
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

// Single task card
function SingleTaskCard({ tool }: { tool: ToolViewProps['tool'] }) {
    const task = tool.input?.task as string;
    let result: SingleResult | null = null;

    if (tool.result) {
        try {
            result = typeof tool.result === 'string' ? JSON.parse(tool.result) : tool.result;
        } catch (_) {}
    }

    return (
        <View style={styles.container}>
            <Text style={styles.taskText} numberOfLines={3}>{task}</Text>
            {result?.session_id && (
                <View style={styles.sessionRow}>
                    <Ionicons name="link-outline" size={12} color="#8E8E93" />
                    <Text style={styles.sessionId}>{result.session_id}</Text>
                </View>
            )}
        </View>
    );
}

// DAG view: SVG edges + absolute-positioned RN nodes
function TaskGraphView({ tool }: ToolViewProps) {
    const tasks: TaskNode[] = tool.input?.tasks || [];
    let result: GraphResult | null = null;

    if (tool.result) {
        try {
            result = typeof tool.result === 'string' ? JSON.parse(tool.result) : tool.result;
        } catch (_) {}
    }

    if (tasks.length === 0) return null;

    const layers = computeLayers(tasks);
    const rootIds = new Set(tasks.filter(t => !t.depends_on || t.depends_on.length === 0).map(t => t.id));
    const positions = buildLayout(layers);

    // Canvas size
    let maxX = 0, maxY = 0;
    positions.forEach(p => {
        maxX = Math.max(maxX, p.x + NODE_W);
        maxY = Math.max(maxY, p.y + NODE_H);
    });
    const canvasW = maxX + PAD;
    const canvasH = maxY + PAD;

    // Edges
    const edges: { from: NodePos; to: NodePos }[] = [];
    for (const task of tasks) {
        const toPos = positions.get(task.id);
        if (!toPos) continue;
        for (const dep of task.depends_on || []) {
            const fromPos = positions.get(dep);
            if (fromPos) edges.push({ from: fromPos, to: toPos });
        }
    }

    return (
        <View style={styles.container}>
            {result && (
                <View style={styles.graphHeader}>
                    <View style={styles.statPill}>
                        <Octicons name="tasklist" size={12} color={C_ROOT} />
                        <Text style={styles.statText}>{result.total_tasks} tasks</Text>
                    </View>
                    <View style={styles.statPill}>
                        <Octicons name="play" size={12} color="#34C759" />
                        <Text style={styles.statText}>{result.root_tasks_started} parallel</Text>
                    </View>
                </View>
            )}

            {/* DAG canvas */}
            <View style={{ width: canvasW, height: canvasH }}>
                {/* SVG layer: edges only */}
                <Svg
                    width={canvasW}
                    height={canvasH}
                    style={{ position: 'absolute', left: 0, top: 0 }}
                >
                    <Defs>
                        <Marker
                            id="arrow"
                            markerWidth="8"
                            markerHeight="6"
                            refX="8"
                            refY="3"
                            orient="auto"
                        >
                            <Polygon
                                points="0,0 8,3 0,6"
                                fill={C_EDGE}
                                opacity={0.5}
                            />
                        </Marker>
                    </Defs>
                    {edges.map((e, i) => {
                        const x1 = e.from.cx;
                        const y1 = e.from.botY;
                        const x2 = e.to.cx;
                        const y2 = e.to.topY;
                        const dy = (y2 - y1) * 0.5;

                        return (
                            <Path
                                key={i}
                                d={`M${x1},${y1} C${x1},${y1 + dy} ${x2},${y2 - dy} ${x2},${y2}`}
                                stroke={C_EDGE}
                                strokeWidth={1.5}
                                fill="none"
                                opacity={0.4}
                                markerEnd="url(#arrow)"
                            />
                        );
                    })}
                </Svg>

                {/* RN layer: nodes */}
                {tasks.map(task => {
                    const p = positions.get(task.id);
                    if (!p) return null;
                    const isRoot = rootIds.has(task.id);
                    const color = isRoot ? C_ROOT : C_DOWN;

                    return (
                        <View
                            key={task.id}
                            style={[
                                dagStyles.node,
                                {
                                    position: 'absolute',
                                    left: p.x,
                                    top: p.y,
                                    width: NODE_W,
                                    height: NODE_H,
                                    borderColor: color + '50',
                                    backgroundColor: color + '14',
                                },
                            ]}
                        >
                            <Text
                                style={[dagStyles.label, { color }]}
                                numberOfLines={1}
                            >
                                {task.id}
                            </Text>
                        </View>
                    );
                })}
            </View>
        </View>
    );
}

export const DispatchTaskView = (props: ToolViewProps) => {
    const { tool } = props;

    if (tool.input?.tasks && Array.isArray(tool.input.tasks)) {
        return <TaskGraphView {...props} />;
    }

    return <SingleTaskCard tool={tool} />;
};

const dagStyles = StyleSheet.create((theme) => ({
    node: {
        justifyContent: 'center',
        alignItems: 'center',
        borderRadius: 6,
        borderWidth: 1,
        paddingHorizontal: 10,
    },
    label: {
        fontSize: 12,
        fontWeight: '600',
        fontFamily: Platform.select({ ios: 'Menlo', default: 'monospace' }),
    },
}));

const styles = StyleSheet.create((theme) => ({
    container: {
        paddingHorizontal: 12,
        paddingVertical: 8,
        gap: 8,
    },
    taskText: {
        fontSize: 13,
        color: theme.colors.text,
        lineHeight: 18,
    },
    sessionRow: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 4,
    },
    sessionId: {
        fontSize: 11,
        color: '#8E8E93',
        fontFamily: Platform.select({ ios: 'Menlo', default: 'monospace' }),
    },
    graphHeader: {
        flexDirection: 'row',
        gap: 8,
    },
    statPill: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 4,
        backgroundColor: theme.colors.surfaceHigh,
        paddingHorizontal: 8,
        paddingVertical: 4,
        borderRadius: 12,
    },
    statText: {
        fontSize: 12,
        color: theme.colors.text,
        fontWeight: '500',
    },
}));
