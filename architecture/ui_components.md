<!-- markdownlint-disable -->
# Epistle — UI Component Spec

**Version:** 0.1 (Outline)  
**Status:** To be detailed as components are built  
**Date:** 2026-04-08  
**Companion Documents:** `gnome_mail_architecture.md`, `v1_scope.md`

---

## Approach

This document will evolve as the UI is built. Widget trees and navigation flows are best documented from working code, not from speculation. The outline captures the known layout model and component inventory from the architecture doc.

---

## 1. Layout Model

- `AdwNavigationSplitView` — three-pane adaptive layout
- Sidebar (210px): unified inbox, per-account folder trees, account switcher
- Message list (280px): conversation rows with avatars, unread pills, account pips
- Content pane (flex): thread view or compose window
- Collapses to single-pane drill-down below 600px

## 2. Component Inventory

### 2.1 App Shell

- `AdwApplicationWindow` — main window
- `AdwNavigationSplitView` — pane management
- `AdwHeaderBar` — three sections aligned with panes
- `AdwToastOverlay` — toast notifications (undo send, undo archive/delete)
- _To be detailed — covers header bar layout, action placement, compose mode transformation._

### 2.2 Sidebar

- _To be detailed — covers folder tree widget, unified inbox row, account sections with pip indicators, unread counts._

### 2.3 Message List

- _To be detailed — covers conversation row widget (GtkListBox or GtkColumnView), avatar stacks, preview text, date formatting, unread styling._

### 2.4 Thread View

- `GtkScrolledWindow` containing `GtkBox` (vertical)
- Message cards: collapsed (native GTK) and expanded (GTK header + WebKitWebView body)
- _To be detailed — covers message card widget, expand/collapse behaviour, WebView lifecycle, action buttons (reply, forward, etc.)._

### 2.5 Compose Window

- Bottom panel mode + expandable to full content pane
- _To be detailed — covers compose layout, toolbar, chip input placement, attachment chips, draft status indicator._

### 2.6 Settings

- `AdwPreferencesWindow`
- _To be detailed — covers accounts page (link to GNOME Settings), signatures page, notifications page._

## 3. Custom Widgets

All custom widgets use GObject subclassing via `glib::subclass` (`ObjectImpl`, `WidgetImpl`, and additional trait impls as needed). This is non-trivial boilerplate in GTK4-rs but is the same pattern used throughout Moments. Blueprint UI templates are used via `#[template(resource = "...")]` composite templates.

**Priority custom widgets** (spike early in Phase 1 alongside layout work):

| Widget | Complexity | Notes |
|---|---|---|
| **Chip input** (To/CC/BCC) | High | `GtkFlowBox` + `GtkEntry` + `GtkPopover` autocomplete. Most complex custom widget. |
| **Message card** (thread view) | Medium | Collapsed (native GTK) vs expanded (WebKitWebView). Manages WebView lifecycle. |
| **Conversation row** (message list) | Medium | Avatar stack, preview text, unread pill, account pip. |
| **Invite card** (calendar) | Medium | Native GTK card with event details and RSVP buttons. |

_To be detailed — full inventory of custom GObject subclasses, signal definitions, property bindings._

## 4. State Management

_To be detailed — covers event flow from sync engine to UI, what triggers list refreshes, how thread view updates on new messages, selection state._

## 5. Keyboard Navigation

| Key | Action |
|---|---|
| `j` / `k` | Navigate message list |
| `e` | Archive |
| `#` | Delete |
| `r` | Reply |
| `a` | Reply All |
| `f` | Forward |
| `c` | Compose |
| `/` | Focus search |
| `?` | Shortcut overlay |

_To be detailed — covers GtkShortcutController setup, action mapping, shortcut overlay widget._

## 6. Empty States & Error Banners

- Empty inbox: `AdwStatusPage` with illustration
- Empty search: `AdwStatusPage`
- Empty folder: `AdwStatusPage`
- Offline: `AdwBanner`
- Account needs re-auth: `AdwBanner` with action button to GNOME Settings
- _To be detailed — covers specific copy, illustrations, banner positioning._

## 7. Adaptive Behaviour

_To be detailed — covers breakpoints, what changes at each width, Steam Deck / mobile considerations._

---

_This document is a living spec. It will be detailed as UI components are built, starting in Phase 1 (basic three-pane layout) and continuing through Phase 4 (polish)._
