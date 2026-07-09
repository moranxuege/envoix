## Title

Polish Receive Destination UX on Apple Platforms

## Problem

The major receive-destination problem has already been addressed in the Apple app:

- macOS defaults to Downloads;
- macOS lets the user choose a folder;
- iOS asks the user to choose a Files folder before receiving;
- iOS stores a bookmark and keeps security-scoped access during transfer;
- completed receives expose a file URL;
- macOS can reveal the file in Finder.

The remaining work is polish and edge-case handling, not a core architecture gap.

## Existing Implementation Found

Already present:

- `ReceiveView.outputDir` resolves Downloads on macOS.
- `FolderPickerSheet` uses `UIDocumentPickerViewController` on iOS.
- `makeSecurityScopedFolderBookmark` and `resolveSecurityScopedFolderBookmark` exist.
- `SecurityScopedResourceAccess` keeps iOS Files access alive during transfer.
- `TransferViewModel.completedFileURL` points to the received file.
- `Support.completedFileControls(...)` offers reveal/open and copy path.
- README documents choosing Downloads in Files on iOS.

Missing or incomplete:

- iOS "Open File" behavior may not be reliable for all security-scoped locations;
- iOS displays Unix-style paths that are not useful to most users;
- stale bookmark recovery should offer a direct "Choose Again" action;
- multi-file and directory receives will need folder-level completion UI;
- conflict results from `ManifestV1` will need visible skipped/renamed files;
- completed-file controls should be driven by transfer records after Activity redesign.

## Goal

Polish Apple receive destination behavior without moving platform-specific file picker logic into the transfer core.

The core should keep owning safe write semantics. Apple should own the native Files/Finder experience.

## Required Changes

### 1. Improve iOS completed-file actions

Evaluate whether `UIApplication.shared.open(url)` is reliable for files inside security-scoped folders selected through Files.

If not, use a more appropriate native flow such as:

- document interaction controller;
- share sheet;
- opening the parent Files location when possible.

### 2. Improve iOS path display

Do not emphasize Unix paths on iOS.

Prefer:

- file name;
- selected folder display name;
- "Open in Files" or "Share" actions;
- copy path only as a secondary diagnostic action, if useful.

### 3. Bookmark expiration recovery

When the saved folder bookmark is stale or inaccessible:

- show a clear message;
- provide a direct Choose Again action;
- do not leave the receiver in a confusing failed state.

### 4. Align with manifest receive results

After `ManifestV1`, completion UI should handle:

- one file;
- multiple files;
- directory transfer;
- renamed conflicts;
- skipped identical files;
- failed files.

### 5. Move detailed receive result display to Activity

Once transfer records exist, Sender and Receiver tabs should show compact status and link to Activity for detailed file results.

## System Boundary

This issue is Apple-specific UI polish.

Shared cross-platform concerns belong elsewhere:

- conflict policy belongs in `ManifestV1`;
- completion semantics belong in reliable transfer completion;
- transfer records belong in Activity/queue;
- structured errors belong in the diagnostics pipeline.

Android will need its own Storage Access Framework / MediaStore design later, but it should consume the same shared transfer result model.

## Dependencies

GitHub issue: #44

Hard dependencies:

- None for the basic Apple-specific polish items.

Full-scope dependencies:

- #38 Structured Error Model and Diagnostics Pipeline, for bookmark-expired and permission-denied recovery actions.
- Transfer Manifest v1, for multi-file, directory, skipped-file, and renamed-file receive results.
- Apple Activity transfer records, for detailed file result display outside the Receiver tab.

## Out of Scope

- Changing core receive path safety
- Implementing Android storage UI
- Multi-file transfer implementation
- Conflict policy design
- Persistent transfer queue

## Acceptance Criteria

- iOS completed receives have reliable user-facing open/share behavior.
- iOS receive result display does not rely primarily on Unix paths.
- Stale folder permission recovery lets the user choose again directly.
- Apple UI can display a folder-level result for future multi-file transfers.
- Existing macOS Downloads/default-folder behavior remains unchanged.
- Existing iOS Files folder selection remains unchanged.

## Follow-up Issues

- Android receive destination UX.
- Manifest receive result UI.
- Activity-based file result details.
