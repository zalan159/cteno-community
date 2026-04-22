//! Agent Message Queue System
//!
//! Manages message queues for agent sessions to support:
//! - User messages queuing while agent is processing
//! - Priority-based message processing

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

/// Message priority levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum MessagePriority {
    Low = 0,    // SubAgent notifications, system messages
    Normal = 1, // Regular user messages
    High = 2,   // Urgent user requests, interrupts
}

/// Agent message in queue
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    pub id: String,
    pub session_id: String,
    pub role: String, // "user" | "system" | "subagent"
    pub content: String,
    pub local_id: Option<String>,
    pub timestamp: i64,
    pub priority: MessagePriority,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

impl AgentMessage {
    /// Create a new user message
    pub fn user(session_id: String, content: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            session_id,
            role: "user".to_string(),
            content,
            local_id: None,
            timestamp: Utc::now().timestamp(),
            priority: MessagePriority::Normal,
            metadata: None,
        }
    }

    /// Create a user message with image attachments
    pub fn user_with_images(
        session_id: String,
        content: String,
        images: Vec<serde_json::Value>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            session_id,
            role: "user".to_string(),
            content,
            local_id: None,
            timestamp: Utc::now().timestamp(),
            priority: MessagePriority::Normal,
            metadata: Some(serde_json::json!({ "images": images })),
        }
    }

    /// Create a system message
    pub fn system(session_id: String, content: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            session_id,
            role: "system".to_string(),
            content,
            local_id: None,
            timestamp: Utc::now().timestamp(),
            priority: MessagePriority::Low,
            metadata: None,
        }
    }

    /// Create a SubAgent notification message
    pub fn subagent(session_id: String, content: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            session_id,
            role: "subagent".to_string(),
            content,
            local_id: None,
            timestamp: Utc::now().timestamp(),
            priority: MessagePriority::Low,
            metadata: None,
        }
    }
}

/// Agent message queue manager
pub struct AgentMessageQueue {
    queues: Arc<Mutex<HashMap<String, VecDeque<AgentMessage>>>>,
    processing: Arc<Mutex<HashMap<String, bool>>>,
}

impl AgentMessageQueue {
    /// Create a new message queue manager
    pub fn new() -> Self {
        Self {
            queues: Arc::new(Mutex::new(HashMap::new())),
            processing: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Push a message to the queue
    pub fn push(&self, message: AgentMessage) -> Result<usize, String> {
        let mut queues = self.queues.lock().unwrap();
        let queue = queues.entry(message.session_id.clone()).or_default();

        // Insert based on priority (higher priority first)
        let insert_pos = queue
            .iter()
            .position(|m| m.priority < message.priority)
            .unwrap_or(queue.len());

        queue.insert(insert_pos, message);
        Ok(queue.len())
    }

    /// Pop the next message from the queue
    pub fn pop(&self, session_id: &str) -> Option<AgentMessage> {
        let mut queues = self.queues.lock().unwrap();
        queues.get_mut(session_id)?.pop_front()
    }

    /// Pop all messages from the queue (for batch processing)
    pub fn pop_all(&self, session_id: &str) -> Vec<AgentMessage> {
        let mut queues = self.queues.lock().unwrap();
        if let Some(queue) = queues.get_mut(session_id) {
            queue.drain(..).collect()
        } else {
            Vec::new()
        }
    }

    /// Peek at the next message without removing it
    pub fn peek(&self, session_id: &str) -> Option<AgentMessage> {
        let queues = self.queues.lock().unwrap();
        queues.get(session_id)?.front().cloned()
    }

    /// Get queue length for a session
    pub fn len(&self, session_id: &str) -> usize {
        let queues = self.queues.lock().unwrap();
        queues.get(session_id).map(|q| q.len()).unwrap_or(0)
    }

    /// Check if queue is empty
    pub fn is_empty(&self, session_id: &str) -> bool {
        self.len(session_id) == 0
    }

    /// Get all messages in queue (for inspection)
    pub fn list(&self, session_id: &str) -> Vec<AgentMessage> {
        let queues = self.queues.lock().unwrap();
        queues
            .get(session_id)
            .map(|q| q.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Clear queue for a session
    pub fn clear(&self, session_id: &str) {
        let mut queues = self.queues.lock().unwrap();
        queues.remove(session_id);
    }

    /// Mark session as processing
    pub fn set_processing(&self, session_id: &str, processing: bool) {
        let mut proc = self.processing.lock().unwrap();
        if processing {
            proc.insert(session_id.to_string(), true);
        } else {
            proc.remove(session_id);
        }
    }

    /// Check if session is currently processing
    pub fn is_processing(&self, session_id: &str) -> bool {
        let proc = self.processing.lock().unwrap();
        proc.get(session_id).copied().unwrap_or(false)
    }

    /// Get queue statistics
    pub fn stats(&self, session_id: &str) -> QueueStats {
        let queues = self.queues.lock().unwrap();
        let queue = queues.get(session_id);

        let total = queue.map(|q| q.len()).unwrap_or(0);
        let by_priority = if let Some(q) = queue {
            let mut stats = HashMap::new();
            for msg in q.iter() {
                *stats.entry(msg.priority).or_insert(0) += 1;
            }
            stats
        } else {
            HashMap::new()
        };

        QueueStats {
            total,
            processing: self.is_processing(session_id),
            by_priority,
        }
    }
}

impl Default for AgentMessageQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// Queue statistics
#[derive(Debug, Clone, Serialize)]
pub struct QueueStats {
    pub total: usize,
    pub processing: bool,
    pub by_priority: HashMap<MessagePriority, usize>,
}

impl Clone for AgentMessageQueue {
    fn clone(&self) -> Self {
        Self {
            queues: Arc::clone(&self.queues),
            processing: Arc::clone(&self.processing),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_priority_queue() {
        let queue = AgentMessageQueue::new();
        let session_id = "test-session";

        // Add messages with different priorities
        queue
            .push(AgentMessage::user(
                session_id.to_string(),
                "Normal 1".to_string(),
            ))
            .unwrap();
        queue
            .push(AgentMessage::system(
                session_id.to_string(),
                "Low 1".to_string(),
            ))
            .unwrap();

        let mut high_msg = AgentMessage::user(session_id.to_string(), "High 1".to_string());
        high_msg.priority = MessagePriority::High;
        queue.push(high_msg).unwrap();

        queue
            .push(AgentMessage::user(
                session_id.to_string(),
                "Normal 2".to_string(),
            ))
            .unwrap();

        // High priority should come first
        let msg1 = queue.pop(session_id).unwrap();
        assert_eq!(msg1.content, "High 1");

        // Then normal priority (FIFO)
        let msg2 = queue.pop(session_id).unwrap();
        assert_eq!(msg2.content, "Normal 1");

        let msg3 = queue.pop(session_id).unwrap();
        assert_eq!(msg3.content, "Normal 2");

        // Finally low priority
        let msg4 = queue.pop(session_id).unwrap();
        assert_eq!(msg4.content, "Low 1");
    }

    #[test]
    fn test_queue_length() {
        let queue = AgentMessageQueue::new();
        let session_id = "test-session";

        assert_eq!(queue.len(session_id), 0);

        queue
            .push(AgentMessage::user(
                session_id.to_string(),
                "Test".to_string(),
            ))
            .unwrap();
        assert_eq!(queue.len(session_id), 1);

        queue.pop(session_id);
        assert_eq!(queue.len(session_id), 0);
    }
}
