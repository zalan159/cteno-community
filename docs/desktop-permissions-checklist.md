# Desktop Permission Implementation Checklist

## Scope
- Platform: macOS desktop (Tauri + Rust backend)
- Target: startup permission gate + runtime prechecks for agent operations
- MVP permissions: `automation_apple_events`, `full_disk_access`, `accessibility`, `screen_recording`

## Tasks
- [x] Create implementation checklist and file-level mapping
- [x] Implement Rust `permissions` module with:
  - [x] permission kinds/states snapshot API
  - [x] status detection for 4 MVP permissions
  - [x] interactive request API (where supported) + settings deep links
- [x] Wire new Tauri commands in `lib.rs` invoke handler
- [x] Keep compatibility `check_permissions` command backed by new snapshot
- [x] Add runtime shell precheck for permission-sensitive commands
- [x] Add frontend Tauri permission API wrapper (`desktopPermissions.ts`)
- [x] Add reusable permission UI panel component
- [x] Add startup permission gate in authenticated desktop home flow
- [x] Add settings page entry for manual permission remediation
- [x] Validate build/typecheck for changed surfaces
- [x] Update checklist statuses after implementation

## File-level plan
- Backend
  - `apps/desktop/src-tauri/src/permissions.rs` (new)
  - `apps/desktop/src-tauri/src/lib.rs`
  - `apps/desktop/src-tauri/src/tool_executors/shell.rs`
  - `apps/desktop/src-tauri/Info.plist`
- Frontend
  - `apps/desktop/sources/utils/desktopPermissions.ts` (new)
  - `apps/desktop/sources/hooks/useDesktopPermissions.ts` (new)
  - `apps/desktop/sources/components/DesktopPermissionsPanel.tsx` (new)
  - `apps/desktop/sources/app/(app)/index.tsx`
  - `apps/desktop/sources/app/(app)/settings/permissions.tsx` (new)
  - `apps/desktop/sources/app/(app)/_layout.tsx`
  - `apps/desktop/sources/components/SettingsView.tsx`

## Validation notes
- `cargo check` (under `apps/desktop/src-tauri`) passed in this worktree.
- `yarn typecheck` reports many pre-existing type errors in unrelated files/translations; no new errors remained in:
  - `sources/app/(app)/index.tsx`
  - `sources/components/DesktopPermissionsPanel.tsx`
