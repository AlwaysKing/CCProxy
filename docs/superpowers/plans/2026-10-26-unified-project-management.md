# Unified Project Management Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace per-mapping project model subwindows with one main-window project manager that
lists every discovered project and edits Claude/Codex project models directly through two selects.

**Architecture:** Keep the existing flat `projectModelBindings` persistence and safe sync engine.
Build one unified view by merging discovered projects with saved bindings, expose route-effective
exact-mapping candidates, and add one set/clear command that preserves the other client's binding.
The Vue page consumes that view from the main navigation and no longer opens a project subwindow.

**Tech Stack:** Rust, Tauri v2, serde_json, Vue 3, Pinia, Vite.

---

## File Structure

- `src-tauri/src/clients/project_models.rs`: route-effective candidate types and candidate helpers.
- `src-tauri/src/state/project_models.rs`: unified project view, merge logic, set/clear transactions.
- `src-tauri/src/state.rs`: prune sync state and emit changes when the final binding is removed.
- `src-tauri/src/commands.rs`: three unified project commands.
- `src-tauri/src/lib.rs`: register the three commands; remove old project subwindow registrations.
- `src-tauri/tests/project_models.rs`: candidate routing and shadowing tests.
- `src-tauri/tests/project_model_state.rs`: unified view and set/clear behavior tests.
- `src/api/tauriBridge.js`: unified command bridge.
- `src/stores/useProjectModelsStore.js`: unified page state and set/refresh actions.
- `src/views/projects/ProjectsView.vue`: project table with two model selects.
- `src/App.vue`: add “项目管理” before “模型映射”; remove subwindow route.
- `src/views/providers/components/MappingModule.vue`: remove per-row project icon and window call.

### Task 1: Route-Effective Model Candidates

**Files:**
- Modify: `src-tauri/src/clients/project_models.rs`
- Modify: `src-tauri/tests/project_models.rs`

- [ ] **Step 1: Write failing candidate tests**

Change the candidate assertion to structured values and add a shadowing test:

```rust
assert_eq!(candidates.codex[0], ProjectModelCandidate {
    mapping_id: "m-codex".into(),
    model: "codex-alias".into(),
    invalid: false,
});
assert_eq!(candidates.claude[0], ProjectModelCandidate {
    mapping_id: "m-claude".into(),
    model: "claude-alias".into(),
    invalid: false,
});
```

```rust
#[test]
fn model_candidates_follow_first_match_routing() {
    let config = config_from(json!({
        "providers": [{
            "id": "p1", "name": "provider", "enabled": true,
            "baseUrl": "https://example.com", "token": "sk-test",
            "modelGroups": [{
                "id": "g1", "apiFormat": "anthropic",
                "models": [{ "name": "real" }]
            }]
        }],
        "protocols": [{
            "id": "anthropic", "name": "Anthropic", "enabled": true,
            "mappings": [
                { "id": "wild", "enabled": true, "clientModel": "claude-3*",
                  "providerId": "p1", "modelGroupId": "g1",
                  "upstreamProtocol": "anthropic", "realModel": "real" },
                { "id": "shadowed", "enabled": true, "clientModel": "claude-3",
                  "providerId": "p1", "modelGroupId": "g1",
                  "upstreamProtocol": "anthropic", "realModel": "real" },
                { "id": "duplicate-ok", "enabled": true, "clientModel": "claude-free",
                  "providerId": "p1", "modelGroupId": "g1",
                  "upstreamProtocol": "anthropic", "realModel": "real" },
                { "id": "duplicate-shadowed", "enabled": true,
                  "clientModel": "claude-free", "providerId": "p1",
                  "modelGroupId": "g1", "upstreamProtocol": "anthropic",
                  "realModel": "real" }
            ]
        }]
    }));
    let candidates = project_model_candidates(&config);
    assert_eq!(candidates.claude.iter()
        .map(|item| item.mapping_id.as_str())
        .collect::<Vec<_>>(), vec!["duplicate-ok"]);
}
```

- [ ] **Step 2: Run the failing tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test project_models model_candidates -- --nocapture`

Expected: compile error because candidates are still `Vec<String>`.

- [ ] **Step 3: Implement candidate structs and first-match filtering**

```rust
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectModelCandidate {
    pub mapping_id: String,
    pub model: String,
    pub invalid: bool,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectModelCandidates {
    pub codex: Vec<ProjectModelCandidate>,
    pub claude: Vec<ProjectModelCandidate>,
}

fn protocol_candidates(config: &AppConfig, protocol_id: &str) -> Vec<ProjectModelCandidate> {
    let Some(protocol) = config.protocol(protocol_id).filter(|item| item.enabled) else {
        return Vec::new();
    };
    let usable: Vec<_> = protocol.mappings.iter().filter(|mapping| {
        mapping.enabled && config.providers.iter().any(|provider| {
            provider.id == mapping.provider_id
                && provider.enabled
                && provider.model_groups.iter().any(|group| group.id == mapping.model_group_id)
        })
    }).collect();
    let mut result = Vec::new();
    for (index, mapping) in usable.iter().enumerate() {
        if mapping.client_model.is_empty() || mapping.client_model.contains('*') {
            continue;
        }
        if usable[..index]
            .iter()
            .any(|earlier| crate::proxy::router::match_mapping(earlier, &mapping.client_model))
        {
            continue;
        }
        result.push(ProjectModelCandidate {
            mapping_id: mapping.id.clone(),
            model: mapping.client_model.clone(),
            invalid: false,
        });
    }
    result
}
```

- [ ] **Step 4: Run candidate tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test project_models model_candidates -- --nocapture`

Expected: PASS, including wildcard and duplicate shadowing.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/clients/project_models.rs src-tauri/tests/project_models.rs
git commit -m "feat: expose routable project model candidates"
```

### Task 2: Unified View and Set/Clear Transactions

**Files:**
- Modify: `src-tauri/src/state/project_models.rs`
- Modify: `src-tauri/src/state.rs`
- Modify: `src-tauri/tests/project_model_state.rs`

- [ ] **Step 1: Write failing state tests**

Add tests for unified rows, set, clear, identity preservation, and offline clearing:

```rust
#[tokio::test]
async fn unified_view_merges_sources_and_existing_offline_binding() {
    let online = TempDir::new("ccproxy-unified-online");
    let offline = TempDir::new("ccproxy-unified-offline");
    let offline_path = offline.path_string();
    let config = routing_config(vec![json!({
        "id": "b-offline", "path": offline_path,
        "client": "codex", "mappingId": "m1"
    })]);
    let state = test_state(config, &TempDir::new("ccproxy-unified-config"));
    let online_path = std::fs::canonicalize(&online.path).unwrap();
    let online_path = online_path.to_string_lossy().to_string();
    state.cache_project_candidates(vec![candidate(&online_path, true, true)]);
    std::fs::remove_dir_all(&offline.path).unwrap();

    let view = state.unified_project_models_view(false).await;
    assert_eq!(view.projects.len(), 2);
    assert!(view.projects.iter().any(|row| row.path == online_path
        && row.sources.codex && row.sources.claude));
    assert!(view.projects.iter().any(|row| row.path == offline_path
        && row.missing && row.codex_model.as_deref() == Some("codex-a")));
}
```

```rust
#[tokio::test]
async fn set_model_preserves_binding_id_and_clear_keeps_client_file() {
    let project = TempDir::new("ccproxy-unified-set");
    let state = test_state(routing_config(Vec::new()),
        &TempDir::new("ccproxy-unified-set-config"));
    let path = std::fs::canonicalize(&project.path).unwrap()
        .to_string_lossy().to_string();
    state.cache_project_candidates(vec![candidate(&path, true, true)]);
    state.set_project_model(&path, "codex", Some("m1")).await.unwrap();
    let id = state.config_snapshot().settings.project_model_bindings[0].id.clone();
    state.set_project_model(&path, "codex", Some("m1")).await.unwrap();
    assert_eq!(state.config_snapshot().settings.project_model_bindings[0].id, id);
    state.set_project_model(&path, "codex", None).await.unwrap();
    assert!(state.config_snapshot().settings.project_model_bindings.is_empty());
    assert_eq!(std::fs::read_to_string(project.join(".codex/config.toml"))
        .unwrap(), "model = \"codex-a\"\n");
}
```

```rust
#[tokio::test]
async fn stale_cached_directory_cannot_create_a_new_binding() {
    let project = TempDir::new("ccproxy-unified-stale");
    let state = test_state(routing_config(Vec::new()),
        &TempDir::new("ccproxy-unified-stale-config"));
    let path = std::fs::canonicalize(&project.path).unwrap()
        .to_string_lossy().to_string();
    state.cache_project_candidates(vec![candidate(&path, true, false)]);
    std::fs::remove_dir_all(&project.path).unwrap();
    let error = state.set_project_model(&path, "codex", Some("m1"))
        .await.unwrap_err();
    assert!(error.contains("目录不存在"));
}
```

- [ ] **Step 2: Run tests and verify RED**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test project_model_state unified -- --nocapture`

Expected: compile errors because `unified_project_models_view` and `set_project_model` do not exist.

- [ ] **Step 3: Add unified view structs**

```rust
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnifiedProjectRow {
    pub id: String,
    pub path: String,
    pub sources: ProjectSources,
    pub last_activity_ms: Option<i64>,
    pub missing: bool,
    pub codex_mapping_id: Option<String>,
    pub codex_model: Option<String>,
    pub claude_mapping_id: Option<String>,
    pub claude_model: Option<String>,
    pub codex_status: Option<ProjectSyncStatusSnapshot>,
    pub claude_status: Option<ProjectSyncStatusSnapshot>,
    pub codex_trusted: Option<bool>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnifiedProjectModelsView {
    pub candidates: ProjectModelCandidates,
    pub projects: Vec<UnifiedProjectRow>,
    pub codex_daemon: Option<crate::clients::daemon::CodexDaemon>,
}
```

- [ ] **Step 4: Implement merge and view**

Use canonical paths for online rows and lexical absolute paths for offline rows:

```rust
fn lexical_project_path(raw: &str) -> String {
    Path::new(raw.trim()).components().collect::<PathBuf>()
        .to_string_lossy().to_string()
}

fn project_identity(raw: &str) -> String {
    std::fs::canonicalize(raw)
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|_| lexical_project_path(raw))
}
```

Build a `BTreeMap<String, UnifiedProjectRow>` from discovered rows first, then saved bindings.
For each binding, set only that client's mapping/model. Derive current model from its mapping even
when the candidate is disabled. Attach status by binding ID and Codex trust. Sort bound rows first,
then by `lastActivityMs` descending and path ascending. Return global candidates from
`project_model_candidates`.

- [ ] **Step 5: Implement set/clear transaction**

```rust
pub async fn set_project_model(
    self: &Arc<Self>,
    path: &str,
    client: &str,
    mapping_id: Option<&str>,
) -> Result<Option<ProjectSyncStatusSnapshot>, String> {
    if client != "codex" && client != "claude" {
        return Err("未知的客户端类型".to_string());
    }
    let discovered = self.ensure_project_candidates(false).await;
    let _guard = self.config_transaction.lock()
        .map_err(|_| "配置事务锁不可用".to_string())?;
    let mut draft = self.config_snapshot();
    let candidates = project_model_candidates(&draft);
    let allowed_mapping = mapping_id.map(|mapping_id| {
        let list = if client == "codex" { &candidates.codex } else { &candidates.claude };
        list.iter().find(|item| item.mapping_id == mapping_id)
            .ok_or_else(|| "该映射当前不可作为项目默认模型".to_string())
    }).transpose()?;
    let existing = draft.settings.project_model_bindings.iter()
        .position(|binding| binding.client == client && same_project_path(&binding.path, path));
    let Some(mapping) = allowed_mapping else {
        if let Some(index) = existing {
            draft.settings.project_model_bindings.remove(index);
            self.save_project_draft_locked(&draft)?;
        }
        return Ok(None);
    };
    let discovered_path = discovered.iter()
        .find(|project| same_project_path(&project.path, path))
        .filter(|project| Path::new(&project.path).is_dir())
        .map(|project| project.path.clone());
    if existing.is_none() && discovered_path.is_none() {
        return Err("项目目录不在自动发现列表中".to_string());
    }
    if client == "claude" { /* reject another binding with same claude_root */ }
    let target = existing.map(|index| draft.settings.project_model_bindings[index].path.clone())
        .or(discovered_path).ok_or_else(|| "项目目录不可用".to_string())?;
    if let Some(index) = existing {
        draft.settings.project_model_bindings[index].mapping_id = mapping.mapping_id.clone();
    } else {
        draft.settings.project_model_bindings.push(ProjectModelBinding {
            id: create_config_id("b"), path: target, client: client.to_string(),
            mapping_id: mapping.mapping_id.clone(),
        });
    }
    let config = self.save_project_draft_locked(&draft)?;
    Ok(draft.settings.project_model_bindings.iter()
        .find(|binding| binding.client == client
            && same_project_path(&binding.path, path))
        .and_then(|binding| self.project_sync_status.read().ok()
            .and_then(|statuses| statuses.get(&binding.id).cloned())))
}
```

- [ ] **Step 6: Emit change when the last binding is removed**

Update `apply_config_effects` so project sync runs whenever project bindings changed, even when the
resulting list is empty. `sync_all_project_models_locked` already performs no project-file writes
for an empty binding list and prunes stale status entries.

- [ ] **Step 7: Validate the actual bound mapping during sync**

Add a helper that returns the model only when the binding's own `mappingId` is in that client's
final candidate list. Use it in `sync_all_project_models_locked` instead of looking up any exact
mapping by model name:

```rust
fn candidate_model_for_binding(
    config: &AppConfig,
    client: &str,
    mapping_id: &str,
) -> Option<String> {
    let candidates = project_model_candidates(config);
    let list = if client == "codex" { &candidates.codex } else { &candidates.claude };
    list.iter().find(|candidate| candidate.mapping_id == mapping_id)
        .map(|candidate| candidate.model.clone())
}
```

Add a regression where the bound mapping is disabled while a later duplicate exact mapping remains
enabled; the bound row must report `binding_invalid`.

- [ ] **Step 8: Run state tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test project_model_state`

Expected: all tests pass.

- [ ] **Step 9: Commit**

```bash
git add src-tauri/src/state.rs src-tauri/src/state/project_models.rs src-tauri/tests/project_model_state.rs
git commit -m "feat: unify project model state and updates"
```

### Task 3: Replace Project Tauri Commands

**Files:**
- Modify: `src-tauri/src/commands.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Replace command signatures**

```rust
#[tauri::command]
pub async fn get_project_models(
    state: State<'_, Arc<AppState>>,
) -> Result<Value, String> {
    Ok(json!({
        "success": true,
        "view": serde_json::to_value(state.unified_project_models_view(false).await)
            .unwrap_or(Value::Null)
    }))
}

#[tauri::command]
pub async fn set_project_model(
    state: State<'_, Arc<AppState>>,
    path: String,
    client: String,
    mapping_id: Option<String>,
) -> Result<Value, String> {
    match state.set_project_model(path.trim(), &client, mapping_id.as_deref()).await {
        Ok(status) => Ok(json!({ "success": true, "status": status })),
        Err(error) => Ok(error_value(error)),
    }
}
```

`unified_project_models_view` must call `refresh_restart_required()` before reading statuses.
`refresh_project_models` calls `unified_project_models_view(true)`. Remove
`open_project_models_window`, `add_project_bindings`, and `remove_project_binding`.

- [ ] **Step 2: Register only the three commands**

In `src-tauri/src/lib.rs`, register `get_project_models`, `refresh_project_models`, and
`set_project_model`.

- [ ] **Step 3: Compile the backend**

Run: `cargo check --manifest-path src-tauri/Cargo.toml`

Expected: exit 0.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/commands.rs src-tauri/src/lib.rs
git commit -m "feat: expose unified project model commands"
```

### Task 4: Main-Window Project Manager

**Files:**
- Modify: `src/api/tauriBridge.js`
- Modify: `src/stores/useProjectModelsStore.js`
- Modify: `src/views/projects/ProjectsView.vue`
- Modify: `src/App.vue`
- Modify: `src/views/providers/components/MappingModule.vue`

- [ ] **Step 1: Replace bridge methods**

```js
getProjectModels: () => invoke('get_project_models'),
refreshProjectModels: () => invoke('refresh_project_models'),
setProjectModel: (path, client, mappingId) =>
  invoke('set_project_model', { path, client, mappingId }),
```

Add these three methods to the browser fallback event object so direct Vite rendering remains safe:
`onProjectModelsChanged: () => () => {}`.

- [ ] **Step 2: Rewrite the store**

State: `view`, `loading`, `busy`, `notice`, `search`; computed `candidates`, `projects`,
`filteredProjects`. `reload(refresh = false)` calls the matching bridge method and applies the view.
`setModel(project, client, mappingId)` calls `setProjectModel(project.path, client, mappingId || null)`,
reloads, surfaces `status.message`, and returns success.

Return only `view`, `loading`, `busy`, `notice`, `search`, `candidates`, `projects`,
`filteredProjects`, `reload`, and `setModel`.

- [ ] **Step 3: Rewrite the page**

The header has title “项目管理”, explanatory text, and one “刷新项目” button. The table has exactly
three columns: 项目, Claude 默认模型, Codex 默认模型. Each model cell contains an `AppSelect` and that
client’s status badge.

```js
function optionsFor(project, client) {
  const list = store.candidates[client] || []
  const options = [
    { value: '', label: '不设置（停止托管）' },
    ...list.map(item => ({ value: item.mappingId, label: item.model }))
  ]
  const mappingId = project[`${client}MappingId`]
  if (mappingId && !list.some(item => item.mappingId === mappingId)) {
    options.push({
      value: mappingId,
      label: `${project[`${client}Model`]}（当前值，不可重新选择）`,
      disabled: true
    })
  }
  return options
}
```

`onChange` calls `store.setModel(project, client, mappingId)`. Keep the select enabled for offline
existing bindings. Show `sources` as Codex/Claude badges, an unavailable-directory warning, and
status beneath each select. No add/edit/delete controls. Use the project path as the row key.

- [ ] **Step 5: Wire page lifecycle**

On mount, call `store.reload()` and subscribe to `onProjectModelsChanged` with another
`store.reload()`; unsubscribe on unmount. After a failed set command, `setModel` must call `reload()`
before returning so the UI discards the unpersisted select value.

- [ ] **Step 6: Add main navigation and remove subwindow routing**

```js
const VIEW_ITEMS = [
  { key: 'projects', label: '项目管理' },
  { key: 'providers', label: '模型映射' },
  { key: 'logs', label: '请求日志' },
  { key: 'usage', label: '用量统计' },
  { key: 'settings', label: '设置' }
]
```

Render `<ProjectsView v-if="activeView === 'projects'" />`; remove URL query/subwindow logic.

- [ ] **Step 7: Remove mapping-row project entry**

Delete `canManageProjects`, `onOpenProjects`, the folder `IconButton`, and the action column's extra
width only if no longer needed.

- [ ] **Step 8: Build the frontend**

Run: `npm run build`

Expected: Vite build succeeds.

- [ ] **Step 9: Commit**

```bash
git add src/api/tauriBridge.js src/stores/useProjectModelsStore.js \
  src/views/projects/ProjectsView.vue src/App.vue \
  src/views/providers/components/MappingModule.vue
git commit -m "feat: add unified project management page"
```

### Task 5: Full Verification

**Files:**
- Verify all modified files.

- [ ] **Step 1: Check formatting**

Run: `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`

Expected: exit 0.

- [ ] **Step 2: Run all Rust tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`

Expected: all tests pass.

- [ ] **Step 3: Build the frontend**

Run: `npm run build`

Expected: exit 0.

- [ ] **Step 4: Check the final diff**

Run: `git diff --check && git status --short`

Expected: no whitespace errors; only intended files and the pre-existing `.codex/` directory.
