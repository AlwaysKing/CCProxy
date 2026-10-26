# Project Model Bindings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an opt-in CCProxy page that discovers project working directories and synchronizes their Codex and Claude Code default model aliases.

**Architecture:** Persist project bindings in `AppState` configuration, merge them with runtime-discovered project paths, and write only the client-specific top-level `model` field through the existing byte-preserving/atomic file helpers. All project mutations and synchronization run under one configuration transaction lock.

**Tech Stack:** Rust, Tauri v2, serde/serde_json, Vue 3, Pinia, Vitest-free Rust integration tests, Vite.

---

### Task 1: Add v6 project binding configuration

**Files:**
- Modify: `src-tauri/src/config.rs`
- Modify: `src-tauri/tests/config_manager.rs`

- [ ] **Step 1: Write failing config tests**

Add tests proving that missing v5 project settings default to disabled/empty, valid bindings round-trip, offline absolute paths survive normalization, duplicate paths are dropped, and duplicate IDs are regenerated.

```rust
#[test]
fn v6_project_models_default_off_and_round_trip() {
    let result = validate_config(&json!({
        "version": 5,
        "settings": {
            "projectModelProjects": [{
                "id": "p1",
                "path": "/tmp/offline-project",
                "codexModel": "codex-model",
                "claudeModel": "",
                "manual": true,
                "hidden": false
            }]
        }
    }));
    let config = result.config.unwrap();
    assert_eq!(config.version, 6);
    assert!(!config.settings.project_model_sync_enabled);
    assert_eq!(config.settings.project_model_projects[0].path, "/tmp/offline-project");
}
```

- [ ] **Step 2: Run the test and verify RED**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test config_manager v6_project_models -- --nocapture`

Expected: compile failure because the v6 settings fields do not exist.

- [ ] **Step 3: Implement the schema and normalization**

Add `ProjectModelProject` and the two `Settings` fields, bump `CONFIG_VERSION` to 6, and normalize them without touching the filesystem.

```rust
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectModelProject {
    pub id: String,
    pub path: String,
    pub codex_model: Option<String>,
    pub claude_model: Option<String>,
    pub manual: bool,
    pub hidden: bool,
}
```

- [ ] **Step 4: Add the v6 migration note**

Split the migration message so v4→v5 mentions mapping reordering while v5→v6 says project model settings were added.

- [ ] **Step 5: Run config tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test config_manager`

Expected: all config tests pass.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/config.rs src-tauri/tests/config_manager.rs
git commit -m "feat: add project model binding config"
```

### Task 2: Add safe root JSON and top-level TOML model writers

**Files:**
- Create: `src-tauri/src/clients/write.rs`
- Create: `src-tauri/tests/project_model_write.rs`
- Modify: `src-tauri/src/clients/apply.rs`
- Modify: `src-tauri/src/clients/json.rs`
- Modify: `src-tauri/src/clients/toml.rs`
- Modify: `src-tauri/src/clients/mod.rs`

- [ ] **Step 1: Write failing writer tests**

Test that Claude root `model` is added/replaced without reordering; Codex top-level `model` is added/replaced while comments and CRLF survive; missing files are created; malformed files are refused; legal triple-quoted `model` is refused without writes; symlinks and modes are preserved.

```rust
#[test]
fn codex_model_update_preserves_other_bytes() {
    let before = "# keep\nmodel = \"old\" # old\n[features]\nmulti_agent = true\n";
    let after = rewrite_codex_model(before, "new\"model").unwrap();
    assert_eq!(after, "# keep\nmodel = \"new\\\"model\" # old\n[features]\nmulti_agent = true\n");
}
```

- [ ] **Step 2: Run the writer tests and verify RED**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test project_model_write`

Expected: compile failure because the writer API does not exist.

- [ ] **Step 3: Extract shared atomic write helpers**

Move `write_file_atomic`, `resolve_target`, `backup_file`, `write_if_changed`, `set_mode`, and `file_mode` from `apply.rs` into `clients/write.rs` without behavior changes.

- [ ] **Step 4: Implement root JSON editing**

Add a root-object variant to `clients/json.rs` and re-parse the result before returning it. Refuse multiline or otherwise unsafe target values.

- [ ] **Step 5: Implement top-level TOML model editing**

Add `rewrite_codex_model(text, model)` and a complete-file parser guard. Encode the model as a TOML basic string. Keep the original bytes outside the target value, including for legal triple-quoted values, and use the full parser only for structural validation.

- [ ] **Step 6: Run writer and existing client tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test project_model_write --test client_apply --test client_json --test client_toml`

Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/clients src-tauri/tests/project_model_write.rs
git commit -m "feat: add safe project model writers"
```

### Task 3: Implement discovery, merge, candidate filtering, and sync service

**Files:**
- Create: `src-tauri/src/clients/project_models.rs`
- Create: `src-tauri/tests/project_models.rs`
- Modify: `src-tauri/src/clients/mod.rs`

- [ ] **Step 1: Write failing discovery and candidate tests**

Use temporary HOME fixtures for Codex session metadata and Claude history/project events. Test path canonicalization, source merge, file-mtime ordering, invalid timestamps, hidden projects, and exact model candidates with valid protocol/provider/model-group references.

```rust
#[test]
fn discovery_merges_codex_and_claude_for_one_project() {
    let project = discover_from("fixtures-one-project");
    assert_eq!(project.len(), 1);
    assert!(project[0].sources.codex);
    assert!(project[0].sources.claude);
}
```

- [ ] **Step 2: Run discovery tests and verify RED**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test project_models discovery_ -- --nocapture`

Expected: compile failure because `clients::project_models` does not exist.

- [ ] **Step 3: Implement discovery and merge**

Read Codex session files until `session_meta.cwd`, use file mtime for activity, read Claude history and fallback project events, normalize timestamps to Unix milliseconds, and deduplicate canonical paths.

- [ ] **Step 4: Implement model candidates**

Return exact, enabled `clientModel` values for `openai_responses` and `anthropic`, excluding wildcard mappings and mappings whose provider/model group is not routable.

- [ ] **Step 5: Implement single-binding apply and status**

Expose a service result containing `ok`, `binding_invalid`, `project_missing`, `config_unreadable`, `write_failed`, or `restart_required`. Never clear an existing model on error.

- [ ] **Step 6: Run discovery/sync tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test project_models`

Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/clients/project_models.rs src-tauri/src/clients/mod.rs src-tauri/tests/project_models.rs
git commit -m "feat: discover and sync project model bindings"
```

### Task 4: Wire state, commands, transaction locking, and background sync

**Files:**
- Modify: `src-tauri/src/state.rs`
- Modify: `src-tauri/src/commands.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/clients/daemon.rs`

- [ ] **Step 1: Write failing state-level sync tests**

Add tests for saving a binding, enabling the feature, mapping changes, disabled-feature no-op, and Codex daemon PID changes clearing `restart_required`.

```rust
#[test]
fn disabling_project_sync_prevents_followup_writes() {
    let state = fixture_state_with_project_binding();
    state.set_project_sync_enabled(false).unwrap();
    state.sync_project_models_after_mapping_change();
    assert!(!fixture_project_config_changed());
}
```

- [ ] **Step 2: Run state tests and verify RED**

Run: `cargo test --manifest-path src-tauri/Cargo.toml project_sync -- --nocapture`

Expected: compile failure for missing state methods.

- [ ] **Step 3: Add transaction and runtime status**

Add one `Mutex` in `AppState`, `project_sync_status: RwLock<HashMap<(String, String), ProjectSyncStatus>>`, and helper methods that lock, re-read current config, mutate, save, and synchronize.

- [ ] **Step 4: Add commands and events**

Implement the seven commands from the spec. Register them in `lib.rs`, emit `project-models-changed` after synchronization, and preserve `config-changed`.

- [ ] **Step 5: Add automatic trigger integration**

In `apply_config_effects`, synchronize when project bindings or relevant protocol/provider/mapping data changes and the feature is enabled. Do not synchronize when disabled.

- [ ] **Step 6: Add Codex daemon PID lifecycle**

Use the global `CODEX_HOME` daemon PID. Record the PID when a project config is written; clear `restart_required` once the daemon stops or its PID changes.

- [ ] **Step 7: Run backend tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`

Expected: all tests pass, or only the known local-socket tests fail under sandbox and pass when rerun with local networking permission.

- [ ] **Step 8: Commit**

```bash
git add src-tauri/src/state.rs src-tauri/src/commands.rs src-tauri/src/lib.rs src-tauri/src/clients/daemon.rs
git commit -m "feat: wire project model synchronization"
```

### Task 5: Build the project model page

**Files:**
- Create: `src/views/projects/ProjectsView.vue`
- Create: `src/stores/useProjectModelsStore.js`
- Modify: `src/api/tauriBridge.js`
- Modify: `src/App.vue`
- Modify: `src/styles/global.css`

- [ ] **Step 1: Add bridge methods and store**

Expose all project-model commands and keep merged rows, candidates, statuses, loading, and notice state in one Pinia store.

- [ ] **Step 2: Build the page**

Add the toggle, search, hidden filter, refresh/add actions, and one row per project with Codex and Claude selects. Show source, status, inaccessible directories, and activation timing.

- [ ] **Step 3: Add navigation and config reload**

Add the top-level “项目模型” tab and reload project rows on `config-changed` and `project-models-changed`.

- [ ] **Step 4: Build the frontend**

Run: `npm run build`

Expected: Vite build succeeds.

- [ ] **Step 5: Commit**

```bash
git add src
git commit -m "feat: add project models page"
```

### Task 6: Full verification and cleanup

**Files:**
- Verify all changed files

- [ ] **Step 1: Format Rust**

Run: `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`

Expected: exit 0.

- [ ] **Step 2: Run the complete Rust suite**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`

Expected: all tests pass. If only local-socket tests are blocked by the sandbox, rerun those tests with networking permission and report both results.

- [ ] **Step 3: Build the frontend**

Run: `npm run build`

Expected: exit 0.

- [ ] **Step 4: Review the final diff**

Run: `git diff main...HEAD --check && git status --short`

Expected: no whitespace errors and no unintended files.

- [ ] **Step 5: Commit any verification-only fixes**

```bash
git add -u
git commit -m "chore: finish project model bindings"
```
