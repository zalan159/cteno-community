# relay-09: Desktop local session message fetch

## meta
- kind: code-review
- severity: high
- scope: apps/client/desktop/src/commands/session.rs, apps/client/app/sync/sync.ts, apps/client/app/sync/storage.ts

## cases

### [pass] Tauri command get_session_messages is registered
- **check**: gui_commands.rs generate_handler macro includes `crate::commands::get_session_messages`
- **expect**: Command is listed in gui_invoke_handler macro
- **evidence**: gui_commands.rs line 37

### [pass] spawn_blocking wraps rusqlite access
- **check**: session.rs get_session_messages uses tauri::async_runtime::spawn_blocking around Connection::open and query
- **expect**: rusqlite calls never block tokio runtime threads
- **evidence**: session.rs lines 67-72

### [pass] Pagination limit/offset parameters work
- **check**: load_session_messages_from_db accepts limit/offset Options, defaults to 100/0
- **expect**: Slices from end of message array, hasMore=true when start>0
- **evidence**: session.rs lines 99-103, unit test lines 138-189

### [pass] Unit test covers pagination from newest
- **check**: Rust unit test paginates_from_newest_message in session.rs
- **expect**: 3 messages, limit=2 offset=0 returns newest 2 with hasMore=true; limit=2 offset=2 returns oldest 1 with hasMore=false
- **evidence**: session.rs lines 133-189

### [pass] Client routing: local machineId triggers local fetch
- **check**: loadVisibleSessionMessages in sync.ts compares session.metadata.machineId to localMachineId
- **expect**: When machineId === localMachineId, calls loadSessionMessagesLocal; otherwise skips
- **evidence**: sync.ts lines 860-870

### [pass] Client loadSessionMessagesLocal invokes Tauri command
- **check**: sync.ts loadSessionMessagesLocal calls invoke('get_session_messages', {sessionId, limit, offset})
- **expect**: Passes through to Tauri backend, normalizes messages, calls applySessionMessagesLocal
- **evidence**: sync.ts lines 809-852

### [pass] Storage action applySessionMessagesLocal exists and sets state
- **check**: storage.ts has applySessionMessagesLocal action that delegates to upsertSessionMessages then sets isLoaded/hasOlderMessages
- **expect**: Session messages state updated with local data, hasMore propagated
- **evidence**: storage.ts lines 637-664

### [pass] Load older messages path uses offset from pagination state
- **check**: sync.ts loadOlderMessages for local sessions uses sessionState.oldestSeq as offset
- **expect**: Incremental pagination loads older messages on scroll
- **evidence**: sync.ts lines 2048-2063

### [pass] Commands module exports session commands
- **check**: commands/mod.rs has `pub mod session` and `pub use session::*`
- **expect**: get_session_messages is re-exported and accessible as crate::commands::get_session_messages
- **evidence**: commands/mod.rs lines 4, 9
