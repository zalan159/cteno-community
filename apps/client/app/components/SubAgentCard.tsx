import React, { useState, useEffect } from 'react';
import { View, Text, TouchableOpacity, StyleSheet } from 'react-native';
import type { SubAgent, SubAgentStatus } from '../sync/ops';

interface SubAgentCardProps {
  subagent: SubAgent;
  onViewDetails: () => void;
  onStop?: () => void;
  onRetry?: () => void;
}

export const SubAgentCard: React.FC<SubAgentCardProps> = ({
  subagent,
  onViewDetails,
  onStop,
  onRetry,
}) => {
  const [time, setTime] = useState(calculateElapsedTime(subagent));

  // 实时更新时间（仅运行中状态）
  useEffect(() => {
    if (subagent.status === 'running') {
      const interval = setInterval(() => {
        setTime(calculateElapsedTime(subagent));
      }, 1000);
      return () => clearInterval(interval);
    }
  }, [subagent.status, subagent.started_at]);

  const formatTime = (seconds: number) => {
    const mins = Math.floor(seconds / 60);
    const secs = seconds % 60;
    if (mins === 0) return `${secs} 秒`;
    if (secs === 0) return `${mins} 分钟`;
    return `${mins} 分 ${secs} 秒`;
  };

  const getStatusConfig = (status: SubAgentStatus) => {
    switch (status) {
      case 'pending':
        return {
          icon: '⏳',
          text: '等待启动',
          color: '#999',
          borderColor: '#ddd',
        };
      case 'running':
        return {
          icon: '🔄',
          text: '运行中',
          color: '#2196F3',
          borderColor: '#2196F3',
        };
      case 'completed':
        return {
          icon: '✅',
          text: '完成',
          color: '#4CAF50',
          borderColor: '#4CAF50',
        };
      case 'failed':
        return {
          icon: '❌',
          text: '失败',
          color: '#f44336',
          borderColor: '#f44336',
        };
      case 'stopped':
        return {
          icon: '⏹️',
          text: '已停止',
          color: '#FF9800',
          borderColor: '#FF9800',
        };
      case 'timed_out':
        return {
          icon: '⏱️',
          text: '超时',
          color: '#FF9800',
          borderColor: '#FF9800',
        };
    }
  };

  const statusConfig = getStatusConfig(subagent.status);
  const displayLabel = subagent.label || subagent.task.substring(0, 50);

  return (
    <View style={[styles.card, { borderColor: statusConfig.borderColor }]}>
      {/* 标题栏 */}
      <View style={styles.header}>
        <Text style={styles.icon}>{statusConfig.icon}</Text>
        <Text style={styles.label} numberOfLines={1}>{displayLabel}</Text>
      </View>

      {/* 状态行 */}
      <View style={styles.statusRow}>
        <Text style={[styles.statusText, { color: statusConfig.color }]}>
          {statusConfig.text}
        </Text>
        <Text style={styles.timeText}> • {formatTime(time)}</Text>
      </View>

      {/* 迭代次数 (运行中显示) */}
      {subagent.status === 'running' && subagent.iteration_count > 0 && (
        <Text style={styles.iterationText}>
          已执行 {subagent.iteration_count} 轮推理
        </Text>
      )}

      {/* 结果摘要 (完成状态) */}
      {subagent.result && subagent.status === 'completed' && (
        <Text style={styles.result} numberOfLines={2}>
          {subagent.result.substring(0, 100)}
          {subagent.result.length > 100 ? '...' : ''}
        </Text>
      )}

      {/* 错误信息 (失败状态) */}
      {subagent.error && (subagent.status === 'failed' || subagent.status === 'timed_out') && (
        <Text style={styles.error} numberOfLines={2}>
          {subagent.error}
        </Text>
      )}

      {/* 操作按钮 */}
      <View style={styles.actions}>
        <TouchableOpacity
          style={styles.button}
          onPress={onViewDetails}
        >
          <Text style={styles.buttonText}>📋 查看详情</Text>
        </TouchableOpacity>

        {subagent.status === 'running' && onStop && (
          <TouchableOpacity
            style={[styles.button, styles.stopButton]}
            onPress={onStop}
          >
            <Text style={styles.buttonText}>⏹️ 停止</Text>
          </TouchableOpacity>
        )}

        {subagent.status === 'failed' && onRetry && (
          <TouchableOpacity
            style={[styles.button, styles.retryButton]}
            onPress={onRetry}
          >
            <Text style={styles.buttonText}>🔄 重试</Text>
          </TouchableOpacity>
        )}
      </View>
    </View>
  );
};

/**
 * 计算已运行时长（秒）
 */
function calculateElapsedTime(subagent: SubAgent): number {
  const now = Date.now();

  if (subagent.status === 'completed' || subagent.status === 'failed' || subagent.status === 'stopped' || subagent.status === 'timed_out') {
    // 已完成任务：使用 completed_at - started_at
    if (subagent.completed_at && subagent.started_at) {
      return Math.floor((subagent.completed_at - subagent.started_at) / 1000);
    }
    if (subagent.started_at) {
      return Math.floor((now - subagent.started_at) / 1000);
    }
    return 0;
  }

  if (subagent.status === 'running') {
    // 运行中：使用 now - started_at
    if (subagent.started_at) {
      return Math.floor((now - subagent.started_at) / 1000);
    }
  }

  // pending 或未启动：使用 now - created_at
  return Math.floor((now - subagent.created_at) / 1000);
}

const styles = StyleSheet.create({
  card: {
    backgroundColor: '#f8f9fa',
    borderRadius: 12,
    borderWidth: 2,
    padding: 16,
    marginVertical: 8,
    marginHorizontal: 12,
  },
  header: {
    flexDirection: 'row',
    alignItems: 'center',
    marginBottom: 8,
  },
  icon: {
    fontSize: 20,
    marginRight: 8,
  },
  label: {
    fontSize: 16,
    fontWeight: '600',
    flex: 1,
  },
  statusRow: {
    flexDirection: 'row',
    alignItems: 'center',
    marginBottom: 4,
  },
  statusText: {
    fontSize: 14,
    fontWeight: '500',
  },
  timeText: {
    fontSize: 14,
    color: '#666',
  },
  iterationText: {
    fontSize: 12,
    color: '#666',
    marginBottom: 8,
    fontStyle: 'italic',
  },
  result: {
    fontSize: 13,
    color: '#4CAF50',
    marginTop: 8,
    marginBottom: 8,
  },
  error: {
    fontSize: 13,
    color: '#f44336',
    marginTop: 8,
    marginBottom: 8,
  },
  actions: {
    flexDirection: 'row',
    gap: 8,
    marginTop: 8,
  },
  button: {
    flex: 1,
    backgroundColor: '#e0e0e0',
    paddingVertical: 8,
    paddingHorizontal: 12,
    borderRadius: 8,
    alignItems: 'center',
  },
  stopButton: {
    backgroundColor: '#ffebee',
  },
  retryButton: {
    backgroundColor: '#fff3e0',
  },
  buttonText: {
    fontSize: 13,
    fontWeight: '500',
  },
});
