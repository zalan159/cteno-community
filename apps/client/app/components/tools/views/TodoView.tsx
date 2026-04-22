import * as React from 'react';
import { View, StyleSheet } from 'react-native';
import { ToolViewProps } from "./_all";
import { knownTools } from '../../tools/knownTools';
import { ToolSectionView } from '../../tools/ToolSectionView';
import { Text } from '@/components/StyledText';

export interface Todo {
    content: string;
    status: 'pending' | 'in_progress' | 'completed';
    priority?: 'high' | 'medium' | 'low';
    id?: string;
}

export const TodoView = React.memo<ToolViewProps>(({ tool }) => {
    let todosList: Todo[] = [];
    let explanation: string | null = null;
    
    const knownTool = knownTools[tool.name as keyof typeof knownTools];

    // Try to get todos from input first
    if (knownTool && 'input' in knownTool && knownTool.input) {
        const parsedArguments = knownTool.input.safeParse(tool.input);
        if (parsedArguments.success) {
            const parsedInput = parsedArguments.data as {
                todos?: Todo[];
                explanation?: string;
            };
            if (Array.isArray(parsedInput.todos)) {
                todosList = parsedInput.todos;
            }
            if (typeof parsedInput.explanation === 'string' && parsedInput.explanation.trim().length > 0) {
                explanation = parsedInput.explanation.trim();
            }
        }
    }
    
    // If we have a properly structured result, use newTodos from there
    let parsed = knownTools.TodoWrite.result.safeParse(tool.result);
    if (parsed.success && parsed.data.newTodos) {
        todosList = parsed.data.newTodos;
    }
    
    // If we have content to display, show it
    if (explanation || todosList.length > 0) {
        return (
            <ToolSectionView>
                <View style={styles.container}>
                    {explanation && (
                        <Text style={styles.explanationText}>
                            {explanation}
                        </Text>
                    )}
                    {todosList.map((todo, index) => {
                        const isCompleted = todo.status === 'completed';
                        const isInProgress = todo.status === 'in_progress';
                        const isPending = todo.status === 'pending';

                        let textStyle: any = styles.todoText;
                        let icon = '☐';

                        if (isCompleted) {
                            textStyle = [styles.todoText, styles.completedText];
                            icon = '☑';
                        } else if (isInProgress) {
                            textStyle = [styles.todoText, styles.inProgressText];
                            icon = '☐';
                        } else if (isPending) {
                            textStyle = [styles.todoText, styles.pendingText];
                        }

                        return (
                            <View key={todo.id || `todo-${index}`} style={styles.todoItem}>
                                <Text style={textStyle}>
                                    {icon} {todo.content}
                                </Text>
                            </View>
                        );
                    })}
                </View>
            </ToolSectionView>
        )
    }

    return null;
});

const styles = StyleSheet.create({
    container: {
        gap: 4,
    },
    todoItem: {
        paddingVertical: 2,
    },
    explanationText: {
        fontSize: 14,
        color: '#444',
        lineHeight: 20,
        marginBottom: 6,
    },
    todoText: {
        fontSize: 14,
        color: '#000',
        flex: 1,
    },
    completedText: {
        color: '#34C759',
        textDecorationLine: 'line-through',
    },
    inProgressText: {
        color: '#007AFF',
    },
    pendingText: {
        color: '#666',
    },
});
